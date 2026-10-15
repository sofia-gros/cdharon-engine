//! 読込、物理更新、退避禁止を分離した正規の活動要求。
//!
//! leaseはdropで消えない保存対象のIDです。解放・期限切れをtick境界の編集で処理し、
//! 必須regionが準備されていない場合に一部分だけactiveへ変更することはありません。

use super::{ChunkCoord, World, WorldEdit, invalid};
use crate::foundation::{
    EngineError, EngineResult, ErrorCode, StableKey,
    codec::{Decoder, Encoder, StateCodec},
    error::allocation,
    id::{Id, WorldId},
    transaction::CandidateState,
};
use std::collections::BTreeSet;

/// 半開矩形のchunk領域。最大端点だけは最終chunk+1を表現できます。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChunkRegion {
    min: ChunkCoord,
    max: [i64; 2],
}
impl ChunkRegion {
    /// 必ず一つ以上のchunkを含む範囲を作ります。
    pub fn new(min: ChunkCoord, max_x: i64, max_y: i64) -> EngineResult<Self> {
        min.validate()?;
        let last = i64::from(i32::MAX / 64) + 1;
        if max_x <= i64::from(min.x) || max_y <= i64::from(min.y) || max_x > last || max_y > last {
            return Err(invalid("chunk領域の端点"));
        }
        Ok(Self {
            min,
            max: [max_x, max_y],
        })
    }
    /// 一つのchunkだけを表します。
    pub fn single(coord: ChunkCoord) -> EngineResult<Self> {
        Self::new(coord, i64::from(coord.x) + 1, i64::from(coord.y) + 1)
    }
    /// 最小chunk。
    pub fn min(self) -> ChunkCoord {
        self.min
    }
    /// 最大の半開端点。
    pub fn max(self) -> [i64; 2] {
        self.max
    }
    /// 対象chunk数。全i32 cell平面でもu64へ収まります。
    pub fn chunk_count(self) -> u64 {
        ((self.max[0] - i64::from(self.min.x)) as u64)
            * ((self.max[1] - i64::from(self.min.y)) as u64)
    }
    /// chunkが領域内か。
    pub fn contains(self, coord: ChunkCoord) -> bool {
        coord.x >= self.min.x
            && coord.y >= self.min.y
            && i64::from(coord.x) < self.max[0]
            && i64::from(coord.y) < self.max[1]
    }
    pub(crate) fn coordinates(self, limit: usize) -> EngineResult<Vec<ChunkCoord>> {
        let count = self.chunk_count();
        if count > limit as u64 {
            return Err(EngineError::capacity(
                "regionのchunk数",
                count,
                limit as u64,
            ));
        }
        let mut result = Vec::new();
        result
            .try_reserve_exact(count as usize)
            .map_err(allocation)?;
        // ChunkCoordと同じ(x,y)順に列挙し、要求順・HashMap順に依存させません。
        for x in i64::from(self.min.x)..self.max[0] {
            for y in i64::from(self.min.y)..self.max[1] {
                result.push(ChunkCoord {
                    x: x as i32,
                    y: y as i32,
                });
            }
        }
        Ok(result)
    }
}
impl StateCodec for ChunkRegion {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        self.min.encode(out)?;
        out.i64(self.max[0])?;
        out.i64(self.max[1])
    }
    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        Self::new(ChunkCoord::decode(input)?, input.i64()?, input.i64()?)
    }
}

/// 正規の活動要求。画面に映るかという情報は含みません。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeaseMode {
    /// 読込状態を保持し、物理は凍結する。
    LoadOnly,
    /// 読込状態を保持し、物理・正規systemの対象にする。
    Simulate,
    /// 読込状態の退避を禁止する。物理の更新要求とは独立する。
    Pin,
}
/// lease用の型識別子。
pub enum LeaseKind {}
/// World内のlease ID。
pub type LeaseId = Id<LeaseKind>;
/// 所属Worldを含むlease参照。値をdropしても要求は解除されません。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LeaseRef {
    /// 所属World。
    pub world: WorldId,
    /// 正規要求の世代付きID。
    pub id: LeaseId,
}
/// 明示解放か期限到達まで保存されるregion要求です。
#[derive(Clone, Debug)]
pub struct RegionLease {
    /// 要求元。容量診断やreplayでも使います。
    pub source: StableKey,
    /// 対象region。
    pub region: ChunkRegion,
    /// 読込・活動・退避禁止の種類。
    pub mode: LeaseMode,
    /// このtickの開始時に解放する。Noneは明示解放まで維持します。
    pub expires: Option<u64>,
}
impl CandidateState for RegionLease {
    fn try_copy(&self) -> EngineResult<Self> {
        Ok(self.clone())
    }
    fn validate(&self) -> EngineResult<()> {
        Ok(())
    }
}
/// Worldの各chunkの準備・活動状態。cameraの可視性とは別です。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegionStatus {
    /// World境界外。
    WorldBoundary,
    /// World内で未準備。
    Unavailable,
    /// 読込済みだが正規更新を凍結中。
    ResidentInactive,
    /// 読込済みでSimulate要求の対象。
    Active,
}
/// 一つのchunkのquery結果。region全体を一つのboolへ丸めません。
#[derive(Clone, Copy, Debug)]
pub struct ChunkCoverage {
    /// 対象座標。
    pub coordinate: ChunkCoord,
    /// 準備・活動状態。
    pub status: RegionStatus,
    /// residentなら地形revision。
    pub revision: Option<u64>,
    /// Pin要求があるか。falseでもLoadOnly/Simulateの保持要求はあり得ます。
    pub pinned: bool,
}
impl World {
    /// 各chunkの準備状態を上限付きで取得します。大量列挙は呼出側が分割できます。
    pub fn region_state(
        &self,
        region: ChunkRegion,
        limit: usize,
    ) -> EngineResult<Vec<ChunkCoverage>> {
        let coords = region.coordinates(limit)?;
        let mut result = Vec::new();
        result.try_reserve_exact(coords.len()).map_err(allocation)?;
        for coordinate in coords {
            let chunk = self.chunks.get(&coordinate);
            let status = if !self.bounds.intersects_chunk(coordinate) {
                RegionStatus::WorldBoundary
            } else {
                match chunk {
                    None => RegionStatus::Unavailable,
                    Some(chunk) if chunk.active => RegionStatus::Active,
                    Some(_) => RegionStatus::ResidentInactive,
                }
            };
            let pinned = self.leases.iter().any(|(_, lease)| {
                lease.mode == LeaseMode::Pin && lease.region.contains(coordinate)
            });
            result.push(ChunkCoverage {
                coordinate,
                status,
                revision: chunk.map(|chunk| chunk.revision),
                pinned,
            });
        }
        Ok(result)
    }
    /// 生存・World所属が一致するleaseを借ります。
    pub fn lease(&self, reference: LeaseRef) -> EngineResult<&RegionLease> {
        if reference.world != self.id {
            return Err(EngineError::new(ErrorCode::WrongScope, "leaseの所属World"));
        }
        self.leases.get(reference.id)
    }
    /// 更新対象のchunkを安定順に走査します。rendererのculling結果は使いません。
    pub fn active_chunks(&self) -> impl Iterator<Item = ChunkCoord> + '_ {
        self.chunks
            .iter()
            .filter_map(|(coord, chunk)| chunk.active.then_some(*coord))
    }
    fn expected_activity(&self) -> EngineResult<BTreeSet<ChunkCoord>> {
        let mut active = BTreeSet::new();
        for (_, lease) in self.leases.iter() {
            if lease.expires.is_some_and(|expires| expires <= self.tick) {
                return Err(invalid("期限切れleaseが残存"));
            }
            for coord in lease
                .region
                .coordinates(self.limits.resident_chunks as usize)?
            {
                if !self.chunks.contains_key(&coord) {
                    return Err(EngineError::new(
                        ErrorCode::InvalidState,
                        "leaseの必須chunkが未準備",
                    )
                    .detail(lease.source.as_str()));
                }
                if lease.mode == LeaseMode::Simulate {
                    active.insert(coord);
                }
            }
        }
        if active.len() > self.limits.active_chunks as usize {
            return Err(EngineError::capacity(
                "lease和のactive chunk数",
                active.len() as u64,
                self.limits.active_chunks.into(),
            ));
        }
        Ok(active)
    }
    pub(crate) fn resolve_activity(&mut self) -> EngineResult<()> {
        let active = self.expected_activity()?;
        for (coord, chunk) in &mut self.chunks {
            chunk.active = active.contains(coord);
        }
        Ok(())
    }
    pub(crate) fn validate_activity(&self) -> EngineResult<()> {
        let active = self.expected_activity()?;
        if self
            .chunks
            .iter()
            .any(|(coord, chunk)| chunk.active != active.contains(coord))
        {
            return Err(invalid("leaseとactive domainの不整合"));
        }
        Ok(())
    }
}
impl WorldEdit<'_> {
    /// 全必須chunkが準備されたregionを保持します。未準備なら候補全体を公開しません。
    pub fn acquire_lease(&mut self, lease: RegionLease) -> EngineResult<LeaseRef> {
        self.run(|editor| {
            if lease
                .expires
                .is_some_and(|expires| expires <= editor.world.tick)
            {
                return Err(invalid("lease期限は未来tickが必要"));
            }
            let id = editor.world.leases.insert(lease)?;
            editor.world.resolve_activity()?;
            Ok(LeaseRef {
                world: editor.world.id,
                id,
            })
        })
    }
    /// leaseを明示解放します。古い参照を使って他の要求を解除できません。
    pub fn release_lease(&mut self, reference: LeaseRef) -> EngineResult<()> {
        self.run(|editor| {
            editor.world.lease(reference)?;
            editor.world.leases.remove(reference.id)?;
            editor.world.resolve_activity()
        })
    }
    /// 一つだけ正規tickを進め、期限切れ要求を解除します。inactiveの地形は変更しません。
    pub fn advance_tick(&mut self) -> EngineResult<()> {
        self.run(|editor| {
            let next = editor
                .world
                .tick
                .checked_add(1)
                .ok_or_else(|| invalid("World tick上限"))?;
            if editor.world.pending_at(editor.world.tick).next().is_some() {
                return Err(EngineError::new(
                    ErrorCode::InvalidState,
                    "必須読込要求の公開待ち",
                ));
            }
            let mut expired = Vec::new();
            expired
                .try_reserve_exact(editor.world.leases.len())
                .map_err(allocation)?;
            for (id, lease) in editor.world.leases.iter() {
                if lease.expires.is_some_and(|expires| expires <= next) {
                    expired.push(id);
                }
            }
            for id in expired {
                editor.world.leases.remove(id)?;
            }
            editor.world.tick = next;
            editor.world.resolve_activity()
        })
    }
}
