//! 自動表示範囲と明示要求のchunk矩形を検証し、容量予約可能な決定的planへ変換します。

use crate::simulation::chunk::{CHUNK_EDGE, ChunkCoord, WORLD_COORD_MAX, WORLD_COORD_MIN};

pub const DEFAULT_MAX_ACTIVE_CHUNKS: usize = 256;
pub const DEFAULT_MAX_RESIDENT_CHUNKS: usize = 512;
pub const HARD_MAX_RESIDENT_CHUNKS: usize = 4_096;
pub const DEFAULT_MAX_PINNED_CHUNKS: usize = 128;
pub const DEFAULT_MAX_PENDING_REQUESTS: usize = 64;
pub const HARD_MAX_PENDING_REQUESTS: usize = 1_024;
pub const HARD_MAX_PINNED_CHUNKS: usize = 4_096;
pub const DEFAULT_CHUNK_BUDGET_BYTES: usize = 256 * 1024 * 1024;
/// 1chunk当たりの保守的予約bytesです。セル、現行scratch、sidecar、管理情報の余白を含めます。
pub const RESERVED_BYTES_PER_CHUNK: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChunkRect {
    pub min: ChunkCoord,
    /// 含まない上端chunk座標です。要求範囲はmin以上max未満の半開区間として解釈します。
    pub max: ChunkCoord,
}

impl ChunkRect {
    pub fn new(min: ChunkCoord, max: ChunkCoord) -> Result<Self, StreamError> {
        let min_chunk = WORLD_COORD_MIN.div_euclid(CHUNK_EDGE);
        let max_chunk_exclusive = WORLD_COORD_MAX.div_euclid(CHUNK_EDGE) + 1;
        if min.x >= max.x
            || min.y >= max.y
            || min.x < min_chunk
            || min.y < min_chunk
            || max.x > max_chunk_exclusive
            || max.y > max_chunk_exclusive
        {
            return Err(StreamError::InvalidRegion);
        }
        Ok(Self { min, max })
    }

    pub fn chunk_count(self) -> Result<usize, StreamError> {
        let width = i64::from(self.max.x) - i64::from(self.min.x);
        let height = i64::from(self.max.y) - i64::from(self.min.y);
        usize::try_from(
            width
                .checked_mul(height)
                .ok_or(StreamError::InvalidRegion)?,
        )
        .map_err(|_| StreamError::InvalidRegion)
    }

    pub fn expand(self, margin: i32) -> Result<Self, StreamError> {
        if margin < 0 {
            return Err(StreamError::InvalidPolicy);
        }
        let min_x = self
            .min
            .x
            .checked_sub(margin)
            .ok_or(StreamError::InvalidRegion)?;
        let min_y = self
            .min
            .y
            .checked_sub(margin)
            .ok_or(StreamError::InvalidRegion)?;
        let max_x = self
            .max
            .x
            .checked_add(margin)
            .ok_or(StreamError::InvalidRegion)?;
        let max_y = self
            .max
            .y
            .checked_add(margin)
            .ok_or(StreamError::InvalidRegion)?;
        Self::new(
            ChunkCoord { x: min_x, y: min_y },
            ChunkCoord { x: max_x, y: max_y },
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadMode {
    Prefetch,
    Simulate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamingPolicy {
    pub offscreen_enabled: bool,
    pub active_margin_chunks: u16,
    pub prefetch_margin_chunks: u16,
    pub max_active_chunks: usize,
    pub max_resident_chunks: usize,
    pub max_pinned_chunks: usize,
    pub max_pending_requests: usize,
    pub chunk_memory_budget_bytes: usize,
}

impl Default for StreamingPolicy {
    fn default() -> Self {
        Self {
            offscreen_enabled: true,
            active_margin_chunks: 1,
            prefetch_margin_chunks: 2,
            max_active_chunks: DEFAULT_MAX_ACTIVE_CHUNKS,
            max_resident_chunks: DEFAULT_MAX_RESIDENT_CHUNKS,
            max_pinned_chunks: DEFAULT_MAX_PINNED_CHUNKS,
            max_pending_requests: DEFAULT_MAX_PENDING_REQUESTS,
            chunk_memory_budget_bytes: DEFAULT_CHUNK_BUDGET_BYTES,
        }
    }
}

impl StreamingPolicy {
    pub fn validate(self) -> Result<Self, StreamError> {
        if self.prefetch_margin_chunks < self.active_margin_chunks
            || self.max_active_chunks == 0
            || self.max_resident_chunks == 0
            || self.max_resident_chunks > HARD_MAX_RESIDENT_CHUNKS
            || self.max_active_chunks > self.max_resident_chunks
            || self.max_pinned_chunks > self.max_resident_chunks
            || self.max_pinned_chunks > HARD_MAX_PINNED_CHUNKS
            || self.max_pending_requests == 0
            || self.max_pending_requests > HARD_MAX_PENDING_REQUESTS
            || self.chunk_memory_budget_bytes < RESERVED_BYTES_PER_CHUNK
        {
            return Err(StreamError::InvalidPolicy);
        }
        let affordable = self.chunk_memory_budget_bytes / RESERVED_BYTES_PER_CHUNK;
        if self.max_resident_chunks > affordable {
            return Err(StreamError::InvalidPolicy);
        }
        Ok(self)
    }

    pub fn plan_visible(self, visible: ChunkRect) -> Result<VisibleChunkPlan, StreamError> {
        self.validate()?;
        let active = if self.offscreen_enabled {
            visible.expand(i32::from(self.active_margin_chunks))?
        } else {
            visible
        };
        let resident = if self.offscreen_enabled {
            visible.expand(i32::from(self.prefetch_margin_chunks))?
        } else {
            visible
        };
        ensure_active_within(active.chunk_count()?, self.max_active_chunks)?;
        ensure_within(resident.chunk_count()?, self.max_resident_chunks)?;
        ensure_memory(resident.chunk_count()?, self.chunk_memory_budget_bytes)?;
        Ok(VisibleChunkPlan { active, resident })
    }

    pub fn plan_request(
        self,
        region: ChunkRect,
        mode: LoadMode,
    ) -> Result<ChunkRequestPlan, StreamError> {
        self.validate()?;
        let chunks = region.chunk_count()?;
        match mode {
            LoadMode::Prefetch => ensure_within(chunks, self.max_resident_chunks)?,
            LoadMode::Simulate => ensure_active_within(chunks, self.max_active_chunks)?,
        }
        ensure_memory(chunks, self.chunk_memory_budget_bytes)?;
        Ok(ChunkRequestPlan {
            region,
            mode,
            chunks,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VisibleChunkPlan {
    pub active: ChunkRect,
    pub resident: ChunkRect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChunkRequestPlan {
    pub region: ChunkRect,
    pub mode: LoadMode,
    pub chunks: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamError {
    InvalidRegion,
    InvalidPolicy,
    ActiveCapacityExceeded,
    ResidentCapacityExceeded,
    MemoryBudgetExceeded,
}

fn ensure_within(actual: usize, limit: usize) -> Result<(), StreamError> {
    if actual > limit {
        return Err(StreamError::ResidentCapacityExceeded);
    }
    Ok(())
}

fn ensure_active_within(actual: usize, limit: usize) -> Result<(), StreamError> {
    if actual > limit {
        return Err(StreamError::ActiveCapacityExceeded);
    }
    Ok(())
}

fn ensure_memory(chunks: usize, budget: usize) -> Result<(), StreamError> {
    let required = chunks
        .checked_mul(RESERVED_BYTES_PER_CHUNK)
        .ok_or(StreamError::MemoryBudgetExceeded)?;
    if required > budget {
        return Err(StreamError::MemoryBudgetExceeded);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(min_x: i32, min_y: i32, max_x: i32, max_y: i32) -> ChunkRect {
        ChunkRect::new(
            ChunkCoord { x: min_x, y: min_y },
            ChunkCoord { x: max_x, y: max_y },
        )
        .unwrap()
    }

    #[test]
    fn default_policy_enables_bounded_offscreen_activity() {
        let plan = StreamingPolicy::default()
            .plan_visible(rect(0, 0, 10, 6))
            .unwrap();
        assert_eq!(plan.active, rect(-1, -1, 11, 7));
        assert_eq!(plan.resident, rect(-2, -2, 12, 8));
        assert!(plan.active.chunk_count().unwrap() < DEFAULT_MAX_ACTIVE_CHUNKS);
    }

    #[test]
    fn creators_can_disable_automatic_margin_but_keep_visible_region() {
        let policy = StreamingPolicy {
            offscreen_enabled: false,
            ..StreamingPolicy::default()
        };
        let plan = policy.plan_visible(rect(0, 0, 10, 6)).unwrap();
        assert_eq!(plan.active, rect(0, 0, 10, 6));
        assert_eq!(plan.resident, rect(0, 0, 10, 6));
    }

    #[test]
    fn explicit_remote_requests_are_preflighted_against_limits() {
        let policy = StreamingPolicy::default();
        let remote = rect(100, -100, 102, -98);
        assert_eq!(
            policy
                .plan_request(remote, LoadMode::Simulate)
                .unwrap()
                .chunks,
            4
        );
        let oversized = rect(-20, -20, 20, 20);
        assert_eq!(
            policy.plan_request(oversized, LoadMode::Simulate),
            Err(StreamError::ActiveCapacityExceeded)
        );
    }

    #[test]
    fn policies_cannot_exceed_hard_or_memory_bounds() {
        let policy = StreamingPolicy {
            chunk_memory_budget_bytes: 1024,
            ..StreamingPolicy::default()
        };
        assert_eq!(policy.validate(), Err(StreamError::InvalidPolicy));
    }

    #[test]
    fn requests_outside_supported_world_coordinate_range_are_rejected() {
        assert_eq!(
            ChunkRect::new(
                ChunkCoord { x: 16_384, y: 0 },
                ChunkCoord { x: 16_385, y: 1 }
            ),
            Err(StreamError::InvalidRegion)
        );
    }
}
