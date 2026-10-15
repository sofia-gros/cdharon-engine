//! World所属とEntity世代を検証する、型付きsparse-set component store。
//!
//! dense配列で値を連続走査し、sparse配列でEntityから位置を引きます。
//! 削除による並び替えを永続形式へ漏らさず、保存はID順に正規化します。

use super::codec::{Decoder, Encoder, StateCodec, invalid};
use super::error::{EngineError, EngineResult, ErrorCode, allocation};
use super::id::{Arena, EntityId, EntityKind, WorldId};
use super::key::StableKey;

const ABSENT: u32 = u32::MAX;

/// ローカルIDとWorld所属を一緒に保持する参照です。
///
/// 別Worldで同じindex/generationが発行されても、その個体へ誤接続しません。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EntityRef {
    world: WorldId,
    local: EntityId,
}

impl EntityRef {
    /// 所属Worldを返します。
    pub fn world(self) -> WorldId {
        self.world
    }
    /// World内のローカルIDを返します。
    pub fn local(self) -> EntityId {
        self.local
    }
}

impl StateCodec for EntityRef {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        self.world.encode(out)?;
        self.local.encode(out)
    }
    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        Ok(Self {
            world: WorldId::decode(input)?,
            local: EntityId::decode(input)?,
        })
    }
}

/// WorldごとのEntity寿命を所有します。ゲーム固有のcomponentは持ちません。
pub struct EntityPool {
    world: WorldId,
    arena: Arena<u8, EntityKind>,
}

impl EntityPool {
    /// 発行元Worldと最大個体数を固定します。WorldIdの発行は上位Runtimeが担当します。
    pub fn new(world: WorldId, capacity: u32) -> EngineResult<Self> {
        Ok(Self {
            world,
            arena: Arena::new(capacity)?,
        })
    }
    /// 新しいEntityを発行します。component追加の取引では候補poolを使います。
    pub fn spawn(&mut self) -> EngineResult<EntityRef> {
        Ok(EntityRef {
            world: self.world,
            local: self.arena.insert(0)?,
        })
    }
    /// 個体を無効化します。component storeは以後その参照を解決しません。
    pub fn despawn(&mut self, entity: EntityRef) -> EngineResult<()> {
        self.validate(entity)?;
        self.arena.remove(entity.local)?;
        Ok(())
    }
    /// 所属と世代を両方確認します。
    pub fn validate(&self, entity: EntityRef) -> EngineResult<()> {
        if entity.world != self.world {
            return Err(wrong_world());
        }
        self.arena.get(entity.local)?;
        Ok(())
    }
    /// 発行元Worldを返します。
    pub fn world(&self) -> WorldId {
        self.world
    }
    /// 最大slot数を返します。
    pub fn capacity(&self) -> u32 {
        self.arena.capacity()
    }
    /// 生存している個体数を返します。
    pub fn len(&self) -> usize {
        self.arena.len()
    }
    /// 生存個体がないかを返します。
    pub fn is_empty(&self) -> bool {
        self.arena.is_empty()
    }
    /// 生存参照をID順に返します。
    pub fn iter(&self) -> impl Iterator<Item = EntityRef> + '_ {
        self.arena.iter().map(|(local, _)| EntityRef {
            world: self.world,
            local,
        })
    }

    /// allocator状態とWorld所属を保存します。
    pub fn checkpoint(&self, byte_limit: usize) -> EngineResult<Vec<u8>> {
        let arena = self.arena.checkpoint(byte_limit)?;
        let mut out = Encoder::new(byte_limit);
        out.raw(b"CHRENT01")?;
        self.world.encode(&mut out)?;
        out.bytes(&arena)?;
        Ok(out.finish())
    }

    /// 指定Worldへ復元します。componentを読む前にpoolを検証して公開できます。
    pub fn restore(
        bytes: &[u8],
        world: WorldId,
        max_slots: u32,
        byte_limit: usize,
    ) -> EngineResult<Self> {
        let mut input = Decoder::new(bytes, byte_limit)?;
        if input.raw(8)? != b"CHRENT01" {
            return Err(invalid());
        }
        if WorldId::decode(&mut input)? != world {
            return Err(wrong_world());
        }
        let arena = Arena::restore(input.bytes(byte_limit)?, max_slots, byte_limit)?;
        input.finish()?;
        // poolのu8は寿命の存在印です。未知の値を新機能として受け付けません。
        if arena.iter().any(|(_, marker)| *marker != 0) {
            return Err(invalid());
        }
        Ok(Self { world, arena })
    }
}

/// 一種類のcomponentを保持します。型名は保存に使わずkey/versionで同定します。
pub struct ComponentStore<T> {
    world: WorldId,
    key: StableKey,
    version: u32,
    sparse: Vec<u32>,
    entities: Vec<EntityId>,
    values: Vec<T>,
    capacity: u32,
}

impl<T> ComponentStore<T> {
    /// sparseとdenseを先に予約します。通常のinsert/removeは再確保しません。
    pub fn new(
        pool: &EntityPool,
        key: StableKey,
        version: u32,
        capacity: u32,
    ) -> EngineResult<Self> {
        if version == 0 || capacity == 0 || capacity > pool.capacity() {
            return Err(EngineError::new(
                ErrorCode::InvalidValue,
                "component容量または版",
            ));
        }
        let mut sparse = Vec::new();
        sparse
            .try_reserve_exact(pool.capacity() as usize)
            .map_err(allocation)?;
        sparse.resize(pool.capacity() as usize, ABSENT);
        let mut entities = Vec::new();
        let mut values = Vec::new();
        entities
            .try_reserve_exact(capacity as usize)
            .map_err(allocation)?;
        values
            .try_reserve_exact(capacity as usize)
            .map_err(allocation)?;
        Ok(Self {
            world: pool.world(),
            key,
            version,
            sparse,
            entities,
            values,
            capacity,
        })
    }

    /// 生存個体へ追加または置換します。再利用slotの旧componentも置換して回収します。
    pub fn insert(
        &mut self,
        pool: &EntityPool,
        entity: EntityRef,
        value: T,
    ) -> EngineResult<Option<T>> {
        self.validate(pool, entity)?;
        let sparse = &mut self.sparse[entity.local.index() as usize];
        if *sparse != ABSENT {
            let index = *sparse as usize;
            self.entities[index] = entity.local;
            return Ok(Some(std::mem::replace(&mut self.values[index], value)));
        }
        if self.values.len() >= self.capacity as usize {
            return Err(EngineError::capacity(
                "component個数",
                self.values.len() as u64 + 1,
                u64::from(self.capacity),
            ));
        }
        *sparse = self.values.len() as u32;
        self.entities.push(entity.local);
        self.values.push(value);
        Ok(None)
    }

    /// 生存と世代が一致するcomponentだけを返します。
    pub fn get(&self, pool: &EntityPool, entity: EntityRef) -> EngineResult<Option<&T>> {
        self.validate(pool, entity)?;
        Ok(self.position(entity.local).map(|index| &self.values[index]))
    }

    /// 正規の構造を変えずに値を一時的に借ります。候補store内での使用を基本とします。
    pub fn get_mut(
        &mut self,
        pool: &EntityPool,
        entity: EntityRef,
    ) -> EngineResult<Option<&mut T>> {
        self.validate(pool, entity)?;
        Ok(self
            .position(entity.local)
            .map(|index| &mut self.values[index]))
    }

    /// componentだけを取り出します。Entity自体の寿命は変更しません。
    pub fn remove(&mut self, pool: &EntityPool, entity: EntityRef) -> EngineResult<Option<T>> {
        self.validate(pool, entity)?;
        Ok(self
            .position(entity.local)
            .map(|index| self.remove_at(index)))
    }

    /// 削除済みEntityのrecordを回収します。構造変更barrierで呼びます。
    ///
    /// getやiterは回収前でも古い値を返しません。未回収recordを含む容量と、
    /// 生存record数を混同しないよう、構造変更の後で明示的に呼びます。
    pub fn compact(&mut self, pool: &EntityPool) -> EngineResult<()> {
        if pool.world() != self.world || pool.capacity() as usize != self.sparse.len() {
            return Err(wrong_world());
        }
        let mut index = 0;
        while index < self.values.len() {
            let entity = EntityRef {
                world: self.world,
                local: self.entities[index],
            };
            if pool.validate(entity).is_err() {
                self.remove_at(index);
            } else {
                index += 1;
            }
        }
        Ok(())
    }

    /// 連続配列の格納順で生存値だけを列挙します。順序依存のSystemはID順を選びます。
    pub fn iter<'a>(
        &'a self,
        pool: &'a EntityPool,
    ) -> EngineResult<impl Iterator<Item = (EntityRef, &'a T)> + 'a> {
        if pool.world() != self.world || pool.capacity() as usize != self.sparse.len() {
            return Err(wrong_world());
        }
        Ok(self
            .entities
            .iter()
            .zip(&self.values)
            .filter_map(move |(local, value)| {
                let entity = EntityRef {
                    world: self.world,
                    local: *local,
                };
                pool.validate(entity).is_ok().then_some((entity, value))
            }))
    }

    /// 生存値をID順で取得します。配列の予約と整列の費用を持つ明示的な経路です。
    pub fn ordered<'a>(&'a self, pool: &'a EntityPool) -> EngineResult<Vec<(EntityRef, &'a T)>> {
        let mut ordered = Vec::new();
        ordered
            .try_reserve_exact(self.values.len())
            .map_err(allocation)?;
        ordered.extend(self.iter(pool)?);
        ordered.sort_unstable_by_key(|(entity, _)| entity.local);
        Ok(ordered)
    }

    fn position(&self, entity: EntityId) -> Option<usize> {
        let index = *self.sparse.get(entity.index() as usize)?;
        (index != ABSENT && self.entities[index as usize] == entity).then_some(index as usize)
    }
    fn validate(&self, pool: &EntityPool, entity: EntityRef) -> EngineResult<()> {
        if pool.world() != self.world || pool.capacity() as usize != self.sparse.len() {
            return Err(wrong_world());
        }
        pool.validate(entity)
    }
    fn remove_at(&mut self, index: usize) -> T {
        let removed = self.entities.swap_remove(index);
        let value = self.values.swap_remove(index);
        self.sparse[removed.index() as usize] = ABSENT;
        if index < self.entities.len() {
            self.sparse[self.entities[index].index() as usize] = index as u32;
        }
        value
    }
}

impl<T: StateCodec> ComponentStore<T> {
    /// 生存値だけをID順に保存し、swap_remove等の履歴を永続形式へ漏らしません。
    pub fn checkpoint(&self, pool: &EntityPool, byte_limit: usize) -> EngineResult<Vec<u8>> {
        let ordered = self.ordered(pool)?;
        let mut out = Encoder::new(byte_limit);
        out.raw(b"CHRCMP01")?;
        self.world.encode(&mut out)?;
        self.key.encode(&mut out)?;
        out.u32(self.version)?;
        out.u32(ordered.len() as u32)?;
        for (entity, value) in ordered {
            entity.local.encode(&mut out)?;
            value.encode(&mut out)?;
        }
        Ok(out.finish())
    }

    /// poolを先に復元し、全IDとschema識別が一致する候補storeだけを返します。
    pub fn restore(
        bytes: &[u8],
        pool: &EntityPool,
        key: StableKey,
        version: u32,
        capacity: u32,
        byte_limit: usize,
    ) -> EngineResult<Self> {
        let mut input = Decoder::new(bytes, byte_limit)?;
        if input.raw(8)? != b"CHRCMP01" {
            return Err(invalid());
        }
        if WorldId::decode(&mut input)? != pool.world() {
            return Err(wrong_world());
        }
        if StableKey::decode(&mut input)? != key || input.u32()? != version {
            return Err(invalid());
        }
        let count = input.u32()?;
        if count > capacity || u64::from(count) * 8 > input.remaining() as u64 {
            return Err(invalid());
        }
        let mut candidate = Self::new(pool, key, version, capacity)?;
        let mut previous = None;
        for _ in 0..count {
            let local = EntityId::decode(&mut input)?;
            if previous.is_some_and(|old| old >= local) {
                return Err(invalid());
            }
            previous = Some(local);
            let value = T::decode(&mut input)?;
            candidate.insert(
                pool,
                EntityRef {
                    world: pool.world(),
                    local,
                },
                value,
            )?;
        }
        input.finish()?;
        Ok(candidate)
    }
}

fn wrong_world() -> EngineError {
    EngineError::new(ErrorCode::WrongScope, "componentまたはEntityのWorld所属")
}

impl super::transaction::CandidateState for EntityPool {
    fn try_copy(&self) -> EngineResult<Self> {
        Ok(Self {
            world: self.world,
            arena: self.arena.try_copy()?,
        })
    }
    fn validate(&self) -> EngineResult<()> {
        self.arena.validate()
    }
}

impl<T: super::transaction::CandidateState> super::transaction::CandidateState
    for ComponentStore<T>
{
    fn try_copy(&self) -> EngineResult<Self> {
        let mut sparse = Vec::new();
        sparse
            .try_reserve_exact(self.sparse.len())
            .map_err(allocation)?;
        sparse.extend_from_slice(&self.sparse);
        let mut entities = Vec::new();
        entities
            .try_reserve_exact(self.capacity as usize)
            .map_err(allocation)?;
        entities.extend_from_slice(&self.entities);
        let mut values = Vec::new();
        values
            .try_reserve_exact(self.capacity as usize)
            .map_err(allocation)?;
        for value in &self.values {
            values.push(value.try_copy()?);
        }
        Ok(Self {
            world: self.world,
            key: self.key,
            version: self.version,
            sparse,
            entities,
            values,
            capacity: self.capacity,
        })
    }
    fn validate(&self) -> EngineResult<()> {
        for value in &self.values {
            value.validate()?;
        }
        Ok(())
    }
}
