//! 再利用後の古い参照を識別できるgenerational EntityIdと、決定的allocator保存状態を提供します。

use std::collections::HashSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EntityId {
    pub index: u32,
    pub generation: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Slot {
    generation: u32,
    live: bool,
    retired: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntityAllocator {
    capacity: u32,
    slots: Vec<Slot>,
    /// 保存対象となる解放slotのLIFO順です。次の確保では最後に解放したslotから再利用します。
    free: Vec<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntityIdError {
    InvalidCapacity,
    CapacityExceeded,
    AllocationFailed,
    InvalidState,
}

impl EntityAllocator {
    pub fn new(capacity: u32) -> Result<Self, EntityIdError> {
        if capacity == 0 {
            return Err(EntityIdError::InvalidCapacity);
        }
        Ok(Self {
            capacity,
            slots: Vec::new(),
            free: Vec::new(),
        })
    }

    pub fn allocate(&mut self) -> Result<EntityId, EntityIdError> {
        if let Some(index) = self.free.pop() {
            let slot = self
                .slots
                .get_mut(index as usize)
                .ok_or(EntityIdError::InvalidState)?;
            if slot.live || slot.retired || slot.generation == 0 {
                return Err(EntityIdError::InvalidState);
            }
            slot.live = true;
            return Ok(EntityId {
                index,
                generation: slot.generation,
            });
        }
        if self.slots.len() >= self.capacity as usize {
            return Err(EntityIdError::CapacityExceeded);
        }
        self.slots
            .try_reserve(1)
            .map_err(|_| EntityIdError::AllocationFailed)?;
        let index = u32::try_from(self.slots.len()).map_err(|_| EntityIdError::CapacityExceeded)?;
        self.slots.push(Slot {
            generation: 1,
            live: true,
            retired: false,
        });
        Ok(EntityId {
            index,
            generation: 1,
        })
    }

    pub fn release(&mut self, id: EntityId) -> Result<bool, EntityIdError> {
        let Some(slot) = self.slots.get_mut(id.index as usize) else {
            return Ok(false);
        };
        if !slot.live || slot.generation != id.generation {
            return Ok(false);
        }
        let Some(generation) = slot.generation.checked_add(1) else {
            slot.live = false;
            slot.retired = true;
            return Ok(true);
        };
        self.free
            .try_reserve(1)
            .map_err(|_| EntityIdError::AllocationFailed)?;
        slot.live = false;
        slot.generation = generation;
        self.free.push(id.index);
        Ok(true)
    }

    pub(crate) fn write_fingerprint(&self, hash: &mut crate::simulation::determinism::Fnv64) {
        hash.write_u32(self.capacity);
        hash.write_u32(self.slots.len() as u32);
        for slot in &self.slots {
            hash.write_u32(slot.generation);
            hash.write(&[u8::from(slot.live) | (u8::from(slot.retired) << 1)]);
        }
        hash.write_u32(self.free.len() as u32);
        for index in &self.free {
            hash.write_u32(*index);
        }
    }

    pub fn contains(&self, id: EntityId) -> bool {
        self.slots
            .get(id.index as usize)
            .is_some_and(|slot| slot.live && !slot.retired && slot.generation == id.generation)
    }

    pub fn capacity(&self) -> u32 {
        self.capacity
    }

    pub fn checkpoint(&self) -> Result<Vec<u8>, EntityIdError> {
        let slot_count =
            u32::try_from(self.slots.len()).map_err(|_| EntityIdError::CapacityExceeded)?;
        let free_count =
            u32::try_from(self.free.len()).map_err(|_| EntityIdError::CapacityExceeded)?;
        let mut bytes = Vec::new();
        let reserve = self
            .slots
            .len()
            .checked_mul(5)
            .and_then(|slots| slots.checked_add(self.free.len().checked_mul(4)?))
            .and_then(|body| body.checked_add(20))
            .ok_or(EntityIdError::CapacityExceeded)?;
        bytes
            .try_reserve_exact(reserve)
            .map_err(|_| EntityIdError::AllocationFailed)?;
        bytes.extend_from_slice(b"CHRID001");
        bytes.extend_from_slice(&self.capacity.to_le_bytes());
        bytes.extend_from_slice(&slot_count.to_le_bytes());
        for slot in &self.slots {
            bytes.extend_from_slice(&slot.generation.to_le_bytes());
            bytes.push(u8::from(slot.live) | (u8::from(slot.retired) << 1));
        }
        bytes.extend_from_slice(&free_count.to_le_bytes());
        for index in &self.free {
            bytes.extend_from_slice(&index.to_le_bytes());
        }
        Ok(bytes)
    }

    pub fn restore(bytes: &[u8]) -> Result<Self, EntityIdError> {
        let mut reader = Reader { bytes, offset: 0 };
        if reader.take(8)? != b"CHRID001" {
            return Err(EntityIdError::InvalidState);
        }
        let capacity = reader.u32()?;
        let slot_count = reader.u32()?;
        if capacity == 0 || slot_count > capacity {
            return Err(EntityIdError::InvalidState);
        }
        let mut slots = Vec::new();
        slots
            .try_reserve_exact(slot_count as usize)
            .map_err(|_| EntityIdError::AllocationFailed)?;
        for _ in 0..slot_count {
            let generation = reader.u32()?;
            let flags = reader.u8()?;
            if generation == 0 || flags & !3 != 0 || flags == 3 {
                return Err(EntityIdError::InvalidState);
            }
            slots.push(Slot {
                generation,
                live: flags & 1 != 0,
                retired: flags & 2 != 0,
            });
        }
        let free_count = reader.u32()?;
        if free_count > slot_count {
            return Err(EntityIdError::InvalidState);
        }
        let mut free = Vec::new();
        free.try_reserve_exact(free_count as usize)
            .map_err(|_| EntityIdError::AllocationFailed)?;
        let mut seen = HashSet::new();
        seen.try_reserve(free_count as usize)
            .map_err(|_| EntityIdError::AllocationFailed)?;
        for _ in 0..free_count {
            let index = reader.u32()?;
            let Some(slot) = slots.get(index as usize) else {
                return Err(EntityIdError::InvalidState);
            };
            if slot.live || slot.retired || !seen.insert(index) {
                return Err(EntityIdError::InvalidState);
            }
            free.push(index);
        }
        if reader.offset != bytes.len()
            || slots
                .iter()
                .enumerate()
                .any(|(index, slot)| !slot.live && !slot.retired && !seen.contains(&(index as u32)))
        {
            return Err(EntityIdError::InvalidState);
        }
        Ok(Self {
            capacity,
            slots,
            free,
        })
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}
impl Reader<'_> {
    fn take(&mut self, len: usize) -> Result<&[u8], EntityIdError> {
        let end = self
            .offset
            .checked_add(len)
            .ok_or(EntityIdError::InvalidState)?;
        let out = self
            .bytes
            .get(self.offset..end)
            .ok_or(EntityIdError::InvalidState)?;
        self.offset = end;
        Ok(out)
    }
    fn u8(&mut self) -> Result<u8, EntityIdError> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> Result<u32, EntityIdError> {
        Ok(u32::from_le_bytes(
            self.take(4)?
                .try_into()
                .map_err(|_| EntityIdError::InvalidState)?,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_ids_do_not_resolve_and_checkpoint_preserves_reuse_order() {
        let mut allocator = EntityAllocator::new(2).unwrap();
        let first = allocator.allocate().unwrap();
        let second = allocator.allocate().unwrap();
        assert_eq!(allocator.allocate(), Err(EntityIdError::CapacityExceeded));
        assert!(allocator.release(first).unwrap());
        assert!(!allocator.contains(first));
        assert!(!allocator.release(first).unwrap());
        let restored = EntityAllocator::restore(&allocator.checkpoint().unwrap()).unwrap();
        assert_eq!(restored, allocator);
        let mut restored = restored;
        let replacement = restored.allocate().unwrap();
        assert_eq!(replacement.index, first.index);
        assert_ne!(replacement.generation, first.generation);
        assert!(!restored.contains(first));
        assert!(restored.contains(second));
    }
}
