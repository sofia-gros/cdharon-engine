//! 大きなcell編集を予算付きで準備し、完成候補だけを公開する正規Job。
//!
//! cursor、入力revision、予約chunk、途中pageを保存します。作業中に入力が変わった場合は
//! Invalidatedを返し、古い候補を現在地形へ混ぜません。prepare中の部分地形はqueryから見えません。

use super::{
    CellCore, CellQuery, ChunkCoord, LayerId, World, WorldEdit, invalid,
    terrain::{ChunkPage, copy_slice, local},
};
use crate::foundation::{
    EngineError, EngineResult, ErrorCode, StableKey,
    codec::{Decoder, Encoder, StateCodec},
    error::allocation,
    grid::CellPoint,
    id::{Arena, Id, WorldId},
    transaction::CandidateState,
};
use std::{collections::BTreeMap, sync::Arc};

/// 一つの格子編集。block個体のcellは個体APIで処理し、直接上書きしません。
#[derive(Clone, Copy, Debug)]
pub struct CellEdit {
    /// 対象layer。
    pub layer: LayerId,
    /// Worldのcell座標。
    pub point: CellPoint,
    /// 検証して置く値。
    pub value: CellCore,
}
/// 大規模編集Jobの型識別子。
pub enum TerrainJobKind {}
/// World内部のJob ID。
pub type TerrainJobId = Id<TerrainJobKind>;
/// 所属Worldも照合するJob参照。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerrainJobRef {
    /// 所属World。
    pub world: WorldId,
    /// 世代付きJob ID。
    pub id: TerrainJobId,
}
/// 部分候補と正規地形を区別する進捗。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerrainJobStatus {
    /// 指定予算の次stepを待っています。
    Preparing,
    /// 全編集の準備が済み、publish待ちです。
    Ready,
    /// 元chunkまたは素材epochが変わりました。候補を公開せず再計画が必要です。
    Invalidated,
    /// 明示取消済みです。
    Cancelled,
    /// 全候補pageを一括公開しました。
    Published,
}
/// 保存対象のJob状態。途中pageは正規地形とは別に所有します。
pub struct TerrainJob {
    pub(crate) source: StableKey,
    pub(crate) edits: Arc<Vec<CellEdit>>,
    pub(crate) cursor: usize,
    pub(crate) epoch: u64,
    pub(crate) revisions: Vec<(ChunkCoord, u64)>,
    pub(crate) staged: BTreeMap<ChunkCoord, Vec<Arc<ChunkPage>>>,
    pub(crate) status: TerrainJobStatus,
}
impl TerrainJob {
    /// 要求元。
    pub fn source(&self) -> StableKey {
        self.source
    }
    /// 準備済みcell編集数。
    pub fn prepared_cells(&self) -> usize {
        self.cursor
    }
    /// 編集総数。終端Jobのpayloadを解放した後は0です。
    pub fn total_cells(&self) -> usize {
        self.edits.len()
    }
    /// 作業の段階。
    pub fn status(&self) -> TerrainJobStatus {
        self.status
    }
    fn bytes(&self) -> usize {
        self.edits.capacity() * size_of::<CellEdit>()
            + self
                .staged
                .values()
                .flatten()
                .map(|page| page.bytes())
                .sum::<usize>()
    }
    fn inputs_match(&self, world: &World) -> bool {
        self.epoch == world.remaps.epoch()
            && self.revisions.iter().all(|(coord, revision)| {
                world
                    .chunks
                    .get(coord)
                    .is_some_and(|chunk| chunk.revision == *revision)
            })
    }
}
impl CandidateState for TerrainJob {
    fn try_copy(&self) -> EngineResult<Self> {
        let mut staged = BTreeMap::new();
        for (coord, pages) in &self.staged {
            staged.insert(*coord, copy_slice(pages)?);
        }
        Ok(Self {
            source: self.source,
            edits: Arc::clone(&self.edits),
            cursor: self.cursor,
            epoch: self.epoch,
            revisions: copy_slice(&self.revisions)?,
            staged,
            status: self.status,
        })
    }
    fn validate(&self) -> EngineResult<()> {
        if self.cursor > self.edits.len()
            || (self.status == TerrainJobStatus::Ready && self.cursor != self.edits.len())
            || self.revisions.windows(2).any(|pair| pair[0].0 >= pair[1].0)
        {
            return Err(invalid("地形Jobの進捗・予約辞書"));
        }
        if self.edits.windows(2).any(|pair| {
            (pair[0].layer, pair[0].point.x, pair[0].point.y)
                >= (pair[1].layer, pair[1].point.x, pair[1].point.y)
        }) {
            return Err(invalid("Job編集辞書の順序または重複"));
        }
        Ok(())
    }
}
impl World {
    // 外部snapshotだけに必要な完全照合です。通常stepではprivateな編集経路が同じ不変条件を
    // 保ちます。復元時はcursor以前の編集を再適用し、保存された候補が別の地形を含まないか確認します。
    pub(crate) fn validate_restored_jobs(&self) -> EngineResult<()> {
        for (_, job) in self.jobs.iter() {
            if !matches!(
                job.status,
                TerrainJobStatus::Preparing | TerrainJobStatus::Ready
            ) {
                continue;
            }
            if job.epoch > self.remaps.epoch() {
                return Err(invalid("Jobの未来material epoch"));
            }
            for (coord, revision) in &job.revisions {
                let current = self
                    .chunks
                    .get(coord)
                    .ok_or_else(|| invalid("予約中Jobのchunkが未読込"))?;
                if *revision > current.revision {
                    return Err(invalid("Jobの未来入力revision"));
                }
            }
            if !job.inputs_match(self) {
                continue;
            }
            let mut expected: BTreeMap<ChunkCoord, Vec<Arc<ChunkPage>>> = BTreeMap::new();
            for edit in &job.edits[..job.cursor] {
                let (coord, index) = local(edit.point);
                if !job.revisions.iter().any(|(reserved, _)| *reserved == coord) {
                    return Err(invalid("Job編集の予約が欠落"));
                }
                if let std::collections::btree_map::Entry::Vacant(entry) = expected.entry(coord) {
                    entry.insert(copy_slice(&self.chunks[&coord].layers)?);
                }
                let page = &mut expected.get_mut(&coord).expect("候補chunkを作成済み")
                    [edit.layer.0 as usize];
                if page
                    .blocks
                    .as_ref()
                    .is_some_and(|blocks| blocks[index].is_some())
                {
                    return Err(invalid("Job編集がblock個体を上書き"));
                }
                if Arc::strong_count(page) > 1 {
                    *page = Arc::new(page.try_copy()?);
                }
                Arc::get_mut(page).expect("照合用の独立page").cells[index] = edit.value;
            }
            if expected.len() != job.staged.len() {
                return Err(invalid("Job候補のcoverage不一致"));
            }
            for (coord, pages) in expected {
                let actual = job
                    .staged
                    .get(&coord)
                    .ok_or_else(|| invalid("Job候補のchunk欠落"))?;
                if pages.len() != actual.len()
                    || pages
                        .iter()
                        .zip(actual)
                        .any(|(a, b)| a.cells != b.cells || a.blocks != b.blocks)
                {
                    return Err(invalid("Job候補が入力とcursorに不一致"));
                }
            }
        }
        Ok(())
    }
    /// Jobを照会します。別World・削除済み世代を拒否します。
    pub fn terrain_job(&self, reference: TerrainJobRef) -> EngineResult<&TerrainJob> {
        if reference.world != self.id {
            return Err(EngineError::new(
                ErrorCode::WrongScope,
                "地形Jobの所属World",
            ));
        }
        self.jobs.get(reference.id)
    }
    /// 準備候補と入力配列の合計保持量。
    pub fn job_bytes(&self) -> usize {
        self.jobs.iter().map(|(_, job)| job.bytes()).sum()
    }
    pub(crate) fn validate_jobs(&self) -> EngineResult<()> {
        if self.job_bytes() > self.limits.job_bytes {
            return Err(EngineError::capacity(
                "地形Job byte数",
                self.job_bytes() as u64,
                self.limits.job_bytes as u64,
            ));
        }
        for (_, job) in self.jobs.iter() {
            job.validate()?;
            if job.edits.len() > self.limits.job_cells {
                return Err(EngineError::capacity(
                    "地形Job cell数",
                    job.edits.len() as u64,
                    self.limits.job_cells as u64,
                ));
            }
            for edit in job.edits.iter() {
                self.content.layer(edit.layer)?;
                edit.value.validate(&self.content)?;
                if !job
                    .revisions
                    .iter()
                    .any(|(coord, _)| *coord == ChunkCoord::at(edit.point))
                {
                    return Err(invalid("Job編集の予約chunkが欠落"));
                }
                if !self.bounds.contains(edit.point) {
                    return Err(invalid("Job編集がWorld境界外"));
                }
            }
            for (coord, pages) in &job.staged {
                if !job.revisions.iter().any(|(reserved, _)| reserved == coord)
                    || pages.len() != self.content.layers.len()
                {
                    return Err(invalid("Job pageの予約・layer数"));
                }
                for page in pages {
                    if page.cells.len() != 4096
                        || page
                            .blocks
                            .as_ref()
                            .is_some_and(|blocks| blocks.len() != 4096)
                    {
                        return Err(invalid("Job pageの寸法"));
                    }
                    for value in &page.cells {
                        value.validate(&self.content)?;
                    }
                }
            }
        }
        Ok(())
    }
    pub(crate) fn job_reserves(&self, coord: ChunkCoord) -> bool {
        self.jobs.iter().any(|(_, job)| {
            matches!(
                job.status,
                TerrainJobStatus::Preparing | TerrainJobStatus::Ready
            ) && job.revisions.iter().any(|(reserved, _)| *reserved == coord)
        })
    }
}
impl WorldEdit<'_> {
    /// 全対象の読込と入力定義を検証してJobを作ります。通常editのcell上限とは別です。
    pub fn submit_cell_job(
        &mut self,
        source: StableKey,
        mut edits: Vec<CellEdit>,
    ) -> EngineResult<TerrainJobRef> {
        self.run(|editor| {
            if edits.is_empty() || edits.len() > editor.world.limits.job_cells {
                return Err(EngineError::capacity(
                    "地形Job cell数",
                    edits.len() as u64,
                    editor.world.limits.job_cells as u64,
                ));
            }
            edits.sort_by_key(|edit| (edit.layer, edit.point.x, edit.point.y));
            if edits
                .windows(2)
                .any(|pair| (pair[0].layer, pair[0].point) == (pair[1].layer, pair[1].point))
            {
                return Err(invalid("Job内の重複cell編集"));
            }
            let mut revisions = BTreeMap::new();
            for edit in &edits {
                edit.value.validate(&editor.world.content)?;
                match editor.world.cell(edit.layer, edit.point)? {
                    CellQuery::Loaded(cell) if cell.block.is_none() => (),
                    _ => {
                        return Err(EngineError::new(
                            ErrorCode::InvalidState,
                            "Job対象は読込済みの非block cellが必要",
                        ));
                    }
                }
                let coord = ChunkCoord::at(edit.point);
                revisions.insert(coord, editor.world.chunks[&coord].revision);
            }
            let mut reserved = Vec::new();
            reserved
                .try_reserve_exact(revisions.len())
                .map_err(allocation)?;
            reserved.extend(revisions);
            let id = editor.world.jobs.insert(TerrainJob {
                source,
                edits: Arc::new(edits),
                cursor: 0,
                epoch: editor.world.remaps.epoch(),
                revisions: reserved,
                staged: BTreeMap::new(),
                status: TerrainJobStatus::Preparing,
            })?;
            editor.world.validate_jobs()?;
            Ok(TerrainJobRef {
                world: editor.world.id,
                id,
            })
        })
    }
    /// 最大budget個の編集を候補pageへ準備します。入力変更ならInvalidatedを保存します。
    pub fn step_cell_job(
        &mut self,
        reference: TerrainJobRef,
        budget: usize,
    ) -> EngineResult<TerrainJobStatus> {
        self.run(|editor| {
            if budget == 0 {
                return Err(invalid("Job step予算は正値"));
            }
            let mut job = editor.world.terrain_job(reference)?.try_copy()?;
            if job.status != TerrainJobStatus::Preparing {
                return Err(EngineError::new(ErrorCode::InvalidState, "準備中でないJob"));
            }
            if !job.inputs_match(editor.world) {
                job.status = TerrainJobStatus::Invalidated;
                job.staged.clear();
                *editor.world.jobs.get_mut(reference.id)? = job;
                return Ok(TerrainJobStatus::Invalidated);
            }
            let end = job.cursor.saturating_add(budget).min(job.edits.len());
            for edit in &job.edits[job.cursor..end] {
                let (coord, index) = local(edit.point);
                if let std::collections::btree_map::Entry::Vacant(entry) = job.staged.entry(coord) {
                    entry.insert(copy_slice(&editor.world.chunks[&coord].layers)?);
                }
                let page = &mut job.staged.get_mut(&coord).expect("候補pageを作成済み")
                    [edit.layer.0 as usize];
                if Arc::strong_count(page) > 1 {
                    *page = Arc::new(page.try_copy()?);
                }
                Arc::get_mut(page).expect("Job専用page").cells[index] = edit.value;
            }
            job.cursor = end;
            if end == job.edits.len() {
                job.status = TerrainJobStatus::Ready;
            }
            let status = job.status;
            *editor.world.jobs.get_mut(reference.id)? = job;
            editor.world.validate_jobs()?;
            Ok(status)
        })
    }
    /// 最後のrevision照合後、準備済みpageの所有権を一括公開します。
    pub fn publish_cell_job(&mut self, reference: TerrainJobRef) -> EngineResult<TerrainJobStatus> {
        self.run(|editor| {
            let mut job = editor.world.terrain_job(reference)?.try_copy()?;
            if job.status != TerrainJobStatus::Ready {
                return Err(EngineError::new(
                    ErrorCode::InvalidState,
                    "公開準備のないJob",
                ));
            }
            if !job.inputs_match(editor.world) {
                job.status = TerrainJobStatus::Invalidated;
                job.staged.clear();
                *editor.world.jobs.get_mut(reference.id)? = job;
                return Ok(TerrainJobStatus::Invalidated);
            }
            for (coord, layers) in std::mem::take(&mut job.staged) {
                let chunk = editor
                    .world
                    .chunks
                    .get_mut(&coord)
                    .expect("予約revisionを照合済み");
                chunk.layers = layers;
                chunk.revision = chunk
                    .revision
                    .checked_add(1)
                    .ok_or_else(|| invalid("Job公開のchunk revision上限"))?;
                chunk.edited = true;
            }
            job.edits = Arc::new(Vec::new());
            job.cursor = 0;
            job.status = TerrainJobStatus::Published;
            *editor.world.jobs.get_mut(reference.id)? = job;
            Ok(TerrainJobStatus::Published)
        })
    }
    /// 正規Jobを取消し、準備pageと入力配列を回収します。
    pub fn cancel_cell_job(&mut self, reference: TerrainJobRef) -> EngineResult<()> {
        self.run(|editor| {
            editor.world.terrain_job(reference)?;
            let job = editor.world.jobs.get_mut(reference.id)?;
            if job.status == TerrainJobStatus::Published {
                return Err(EngineError::new(
                    ErrorCode::Conflict,
                    "公開済みJobは取消不可",
                ));
            }
            job.status = TerrainJobStatus::Cancelled;
            job.edits = Arc::new(Vec::new());
            job.cursor = 0;
            job.staged.clear();
            Ok(())
        })
    }
    /// 終端Jobを解放します。IDの世代を進め、後着参照を無効にします。
    pub fn forget_cell_job(&mut self, reference: TerrainJobRef) -> EngineResult<()> {
        self.run(|editor| {
            if matches!(
                editor.world.terrain_job(reference)?.status,
                TerrainJobStatus::Preparing | TerrainJobStatus::Ready
            ) {
                return Err(EngineError::new(
                    ErrorCode::InvalidState,
                    "処理中Jobは取消が必要",
                ));
            }
            editor.world.jobs.remove(reference.id)?;
            Ok(())
        })
    }
}
impl StateCodec for TerrainJob {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        self.source.encode(out)?;
        out.u64(self.epoch)?;
        out.u64(self.cursor as u64)?;
        out.u8(match self.status {
            TerrainJobStatus::Preparing => 0,
            TerrainJobStatus::Ready => 1,
            TerrainJobStatus::Invalidated => 2,
            TerrainJobStatus::Cancelled => 3,
            TerrainJobStatus::Published => 4,
        })?;
        out.u32(self.edits.len() as u32)?;
        for edit in self.edits.iter() {
            edit.layer.encode(out)?;
            edit.point.encode(out)?;
            edit.value.encode(out)?;
        }
        out.u32(self.revisions.len() as u32)?;
        for (coord, revision) in &self.revisions {
            coord.encode(out)?;
            out.u64(*revision)?;
        }
        out.u32(self.staged.len() as u32)?;
        for (coord, pages) in &self.staged {
            coord.encode(out)?;
            out.u32(pages.len() as u32)?;
            for page in pages {
                super::snapshot::encode_page(page, out)?;
            }
        }
        Ok(())
    }
    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        let source = StableKey::decode(input)?;
        let epoch = input.u64()?;
        let cursor = usize::try_from(input.u64()?).map_err(|_| invalid("Job cursor"))?;
        let status = match input.u8()? {
            0 => TerrainJobStatus::Preparing,
            1 => TerrainJobStatus::Ready,
            2 => TerrainJobStatus::Invalidated,
            3 => TerrainJobStatus::Cancelled,
            4 => TerrainJobStatus::Published,
            _ => return Err(invalid("Job状態")),
        };
        let count = input.u32()? as usize;
        if count > input.remaining() / 20 {
            return Err(invalid("Job編集数"));
        }
        let mut edits = Vec::new();
        edits.try_reserve_exact(count).map_err(allocation)?;
        for _ in 0..count {
            edits.push(CellEdit {
                layer: LayerId::decode(input)?,
                point: CellPoint::decode(input)?,
                value: CellCore::decode(input)?,
            });
        }
        let count = input.u32()? as usize;
        if count > input.remaining() / 16 {
            return Err(invalid("Job予約数"));
        }
        let mut revisions = Vec::new();
        revisions.try_reserve_exact(count).map_err(allocation)?;
        for _ in 0..count {
            revisions.push((ChunkCoord::decode(input)?, input.u64()?));
        }
        let count = input.u32()? as usize;
        if count > input.remaining() / 12 {
            return Err(invalid("Job候補chunk数"));
        }
        let mut staged = BTreeMap::new();
        for _ in 0..count {
            let coord = ChunkCoord::decode(input)?;
            let count = input.u32()? as usize;
            if count > input.remaining() / (4096 * 8 + 1) {
                return Err(invalid("Job候補layer数"));
            }
            let mut pages = Vec::new();
            pages.try_reserve_exact(count).map_err(allocation)?;
            for _ in 0..count {
                pages.push(Arc::new(super::snapshot::decode_page(input)?));
            }
            if staged.insert(coord, pages).is_some() {
                return Err(invalid("Job候補chunk重複"));
            }
        }
        let job = Self {
            source,
            edits: Arc::new(edits),
            cursor,
            epoch,
            revisions,
            staged,
            status,
        };
        job.validate()?;
        Ok(job)
    }
}

pub(crate) fn empty(capacity: u32) -> EngineResult<Arena<TerrainJob, TerrainJobKind>> {
    Arena::new(capacity)
}
