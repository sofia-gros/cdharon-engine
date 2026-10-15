//! 生成処理の実行と、Worldへの採用を分離する境界。
//!
//! GenerationTaskは不変の入力だけを持ち、作者のthreadへ移せます。完了した結果は
//! submitでReadyとなるだけです。作者が指定した正規tickのpublishで初めて地形になります。
//! passは座標とseedから結果を決め、隣接chunkの完了順を参照しません。

use super::{
    CellCore, ChunkCoord, ChunkRegion, Content, LayerId, World, WorldBounds, WorldEdit, conflict,
    invalid,
    terrain::{CELL_COUNT, Chunk, ChunkPage, copy_slice, local},
};
use crate::foundation::{
    EngineError, EngineResult, ErrorCode, StableKey,
    codec::{Encoder, StateCodec},
    error::allocation,
    grid::CellPoint,
    id::{Id, WorldId},
    transaction::CandidateState,
};
use std::{collections::BTreeSet, sync::Arc};

/// 生成passの不変入力。cellと表示pixelを混同しません。
pub struct GenerationInput<'a> {
    /// World seed。
    pub seed: u64,
    /// 対象の絶対chunk座標。
    pub coordinate: ChunkCoord,
    /// 有限または拡張可能なWorld境界。
    pub bounds: WorldBounds,
    /// 確定した素材・layer定義。
    pub content: &'a Content,
}
impl GenerationInput<'_> {
    /// 任意の絶対anchorと乱数domainから独立seedを導きます。
    /// 隣接chunkを同時生成しても、同じ構造物anchorには同じ値が得られます。
    pub fn anchor_seed(&self, domain: StableKey, anchor: CellPoint) -> u64 {
        let mut hash = blake3::Hasher::new();
        hash.update(&self.seed.to_le_bytes());
        hash.update(domain.as_str().as_bytes());
        hash.update(&anchor.x.to_le_bytes());
        hash.update(&anchor.y.to_le_bytes());
        u64::from_le_bytes(
            hash.finalize().as_bytes()[..8]
                .try_into()
                .expect("BLAKE3の先頭8byte"),
        )
    }
}
/// 作者の生成pass。登録順を共有乱数の消費順にしないでください。
pub trait GenerationPass: Send + Sync {
    /// 実装の安定キー。
    fn key(&self) -> StableKey;
    /// 生成結果の互換性を表す版。
    fn version(&self) -> u32;
    /// 先に完了すべきpass。
    fn dependencies(&self) -> &[StableKey];
    /// seed以外の設定を正規byte列で返します。manifestへそのまま含めます。
    fn configuration(&self) -> &[u8];
    /// 指定chunkへclipした地形を作ります。WorldやOS資源への可変参照は受け取りません。
    fn generate(
        &self,
        input: &GenerationInput<'_>,
        output: &mut ChunkDraft<'_>,
    ) -> EngineResult<()>;
}
/// 依存順を確定した不変の生成器。複数workerから共有できます。
pub struct GenerationPipeline {
    key: StableKey,
    version: u32,
    fingerprint: [u8; 32],
    passes: Vec<Arc<dyn GenerationPass>>,
}
impl GenerationPipeline {
    /// 依存を検証し、同順位は安定キー順で解決します。欠落と循環は実行前に拒否します。
    pub fn new(
        key: StableKey,
        version: u32,
        mut passes: Vec<Arc<dyn GenerationPass>>,
        manifest_limit: usize,
    ) -> EngineResult<Arc<Self>> {
        if version == 0 {
            return Err(invalid("生成器の版"));
        }
        passes.sort_by_key(|pass| pass.key());
        if passes.windows(2).any(|v| v[0].key() == v[1].key()) {
            return Err(EngineError::new(ErrorCode::DuplicateKey, "生成pass"));
        }
        let keys: BTreeSet<_> = passes.iter().map(|pass| pass.key()).collect();
        for pass in &passes {
            if pass.version() == 0 {
                return Err(invalid("生成passの版"));
            }
            if pass.dependencies().iter().any(|key| !keys.contains(key)) {
                return Err(EngineError::new(
                    ErrorCode::MissingDependency,
                    "生成pass依存",
                ));
            }
        }
        let mut ordered: Vec<Arc<dyn GenerationPass>> = Vec::new();
        ordered
            .try_reserve_exact(passes.len())
            .map_err(allocation)?;
        let mut done = BTreeSet::new();
        while !passes.is_empty() {
            let index = passes
                .iter()
                .position(|pass| pass.dependencies().iter().all(|key| done.contains(key)))
                .ok_or_else(|| EngineError::new(ErrorCode::DependencyCycle, "生成pass依存"))?;
            let pass = passes.remove(index);
            done.insert(pass.key());
            ordered.push(pass);
        }
        let mut out = Encoder::new(manifest_limit);
        out.raw(b"CHGEN001")?;
        key.encode(&mut out)?;
        out.u32(version)?;
        for pass in &ordered {
            pass.key().encode(&mut out)?;
            out.u32(pass.version())?;
            out.bytes(pass.configuration())?;
            let mut dependencies = copy_slice(pass.dependencies())?;
            dependencies.sort();
            dependencies.dedup();
            out.u32(dependencies.len() as u32)?;
            for dependency in dependencies {
                dependency.encode(&mut out)?;
            }
        }
        Ok(Arc::new(Self {
            key,
            version,
            fingerprint: *blake3::hash(&out.finish()).as_bytes(),
            passes: ordered,
        }))
    }
    /// 生成器キー。
    pub fn key(&self) -> StableKey {
        self.key
    }
    /// 生成器版。
    pub fn version(&self) -> u32 {
        self.version
    }
    /// pass・依存・設定を含むhash。
    pub fn fingerprint(&self) -> [u8; 32] {
        self.fingerprint
    }
}

/// 生成中chunkの書込み窓口。layerと素材は対象ContentのIDを使います。
pub struct ChunkDraft<'a> {
    content: &'a Content,
    bounds: WorldBounds,
    coordinate: ChunkCoord,
    pages: Vec<ChunkPage>,
    error: Option<EngineError>,
}
impl ChunkDraft<'_> {
    /// 対象chunkのcellを設定します。別chunkへの書込や境界外の非空cellを拒否します。
    /// generatorがこのエラーを無視しても、そのchunk全体はReadyになりません。
    pub fn set_cell(
        &mut self,
        layer: LayerId,
        point: CellPoint,
        value: CellCore,
    ) -> EngineResult<()> {
        let result = (|| {
            if let Some(error) = &self.error {
                return Err(error.clone());
            }
            self.content.layer(layer)?;
            value.validate(self.content)?;
            if ChunkCoord::at(point) != self.coordinate {
                return Err(invalid("生成chunk外のcell"));
            }
            if !self.bounds.contains(point) && value != CellCore::AIR {
                return Err(invalid("World境界外の生成"));
            }
            self.pages[layer.0 as usize].cells[local(point).1] = value;
            Ok(())
        })();
        if let Err(error) = &result {
            self.error = Some(error.clone());
        }
        result
    }
    /// 先行passが生成したcellを読みます。別chunkの実行結果にはアクセスしません。
    pub fn cell(&self, layer: LayerId, point: CellPoint) -> EngineResult<CellCore> {
        self.content.layer(layer)?;
        if ChunkCoord::at(point) != self.coordinate {
            return Err(invalid("生成chunk外の参照"));
        }
        Ok(self.pages[layer.0 as usize].cells[local(point).1])
    }
}
/// 生成版と設定の正規識別子。再生成可能性の判定と保存manifestで使います。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GenerationStamp {
    /// 生成器。
    pub key: StableKey,
    /// 生成器版。
    pub version: u32,
    /// pass・設定のhash。
    pub fingerprint: [u8; 32],
}
pub(crate) struct PreparedChunk {
    pub(crate) coordinate: ChunkCoord,
    pub(crate) pages: Vec<Arc<ChunkPage>>,
}
impl PreparedChunk {
    fn try_copy(&self) -> EngineResult<Self> {
        Ok(Self {
            coordinate: self.coordinate,
            pages: copy_slice(&self.pages)?,
        })
    }
}
/// streaming要求用の型識別子。
pub enum RequestKind {}
/// World内の読込要求ID。
pub type RequestId = Id<RequestKind>;
/// 所属Worldを検査する要求参照。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RequestRef {
    /// 所属World。
    pub world: WorldId,
    /// 正規要求ID。
    pub id: RequestId,
}
/// IO/生成の完了状態と、正規Worldへの採用を別の段階にします。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestStatus {
    /// 要求が記録され、まだworkerへ渡していません。
    Requested,
    /// 所有された入力をworkerへ渡しました。
    Preparing,
    /// 検証済み候補が到着し、明示publishを待っています。
    Ready,
    /// 生成器または候補の検証が失敗しました。
    Failed,
    /// 取り消され、遅い結果を拒否します。
    Cancelled,
    /// 正規tickで公開済みです。
    Published,
}
/// 保存・診断へ公開する要求の不変情報。
pub struct RegionRequest {
    pub(crate) source: StableKey,
    pub(crate) region: ChunkRegion,
    pub(crate) stamp: GenerationStamp,
    pub(crate) due_tick: u64,
    pub(crate) attempt: u64,
    pub(crate) content_hash: [u8; 32],
    pub(crate) status: RequestStatus,
    pub(crate) ready: Option<Vec<PreparedChunk>>,
    pub(crate) failure: Option<EngineError>,
}
impl RegionRequest {
    /// 要求元。
    pub fn source(&self) -> StableKey {
        self.source
    }
    /// 対象region。
    pub fn region(&self) -> ChunkRegion {
        self.region
    }
    /// 公開を予定するtick。
    pub fn due_tick(&self) -> u64 {
        self.due_tick
    }
    /// 準備・公開状態。
    pub fn status(&self) -> RequestStatus {
        self.status
    }
    /// 失敗の診断。正規制御はstatusを使い、診断文の文言へ依存しません。
    pub fn failure(&self) -> Option<&EngineError> {
        self.failure.as_ref()
    }
}
impl CandidateState for RegionRequest {
    fn try_copy(&self) -> EngineResult<Self> {
        let ready = if let Some(chunks) = &self.ready {
            let mut copy = Vec::new();
            copy.try_reserve_exact(chunks.len()).map_err(allocation)?;
            for chunk in chunks {
                copy.push(chunk.try_copy()?);
            }
            Some(copy)
        } else {
            None
        };
        Ok(Self {
            source: self.source,
            region: self.region,
            stamp: self.stamp,
            due_tick: self.due_tick,
            attempt: self.attempt,
            content_hash: self.content_hash,
            status: self.status,
            ready,
            failure: self.failure.clone(),
        })
    }
    fn validate(&self) -> EngineResult<()> {
        if (self.status == RequestStatus::Ready) != self.ready.is_some() {
            return Err(invalid("Readyと生成候補の不整合"));
        }
        if (self.status == RequestStatus::Failed) != self.failure.is_some() {
            return Err(invalid("生成失敗と診断区分の不整合"));
        }
        Ok(())
    }
}

/// workerへ移せる不変の要求入力。Worldの可変参照を保持しません。
pub struct GenerationTask {
    request: RequestRef,
    attempt: u64,
    seed: u64,
    bounds: WorldBounds,
    content: Arc<Content>,
    coordinates: Vec<ChunkCoord>,
    stamp: GenerationStamp,
    byte_limit: usize,
}
/// workerの結果。submit前は地形でもactive domainでもありません。
pub struct GenerationResult {
    request: RequestRef,
    attempt: u64,
    content_hash: [u8; 32],
    stamp: GenerationStamp,
    result: EngineResult<Vec<PreparedChunk>>,
}
impl GenerationTask {
    /// 任意threadで生成します。異なるpipelineの結果を成功として返しません。
    pub fn run(self, pipeline: &GenerationPipeline) -> GenerationResult {
        let result = (|| {
            if pipeline.fingerprint != self.stamp.fingerprint
                || pipeline.key != self.stamp.key
                || pipeline.version != self.stamp.version
            {
                return Err(EngineError::new(
                    ErrorCode::MissingDependency,
                    "要求した生成器と版が存在しない",
                ));
            }
            let bytes = self
                .coordinates
                .len()
                .checked_mul(self.content.layers.len())
                .and_then(|v| v.checked_mul(CELL_COUNT * size_of::<CellCore>()))
                .ok_or_else(|| invalid("生成結果byte数overflow"))?;
            if bytes > self.byte_limit {
                return Err(EngineError::capacity(
                    "生成候補byte数",
                    bytes as u64,
                    self.byte_limit as u64,
                ));
            }
            let mut chunks = Vec::new();
            chunks
                .try_reserve_exact(self.coordinates.len())
                .map_err(allocation)?;
            for coordinate in &self.coordinates {
                let mut pages = Vec::new();
                pages
                    .try_reserve_exact(self.content.layers.len())
                    .map_err(allocation)?;
                for _ in &self.content.layers {
                    pages.push(ChunkPage::empty()?);
                }
                let mut draft = ChunkDraft {
                    content: &self.content,
                    bounds: self.bounds,
                    coordinate: *coordinate,
                    pages,
                    error: None,
                };
                let input = GenerationInput {
                    seed: self.seed,
                    coordinate: *coordinate,
                    bounds: self.bounds,
                    content: &self.content,
                };
                for pass in &pipeline.passes {
                    pass.generate(&input, &mut draft)?;
                    if let Some(error) = draft.error {
                        return Err(error);
                    }
                }
                let mut pages = Vec::new();
                pages
                    .try_reserve_exact(draft.pages.len())
                    .map_err(allocation)?;
                for page in draft.pages {
                    pages.push(Arc::new(page));
                }
                chunks.push(PreparedChunk {
                    coordinate: *coordinate,
                    pages,
                });
            }
            Ok(chunks)
        })();
        GenerationResult {
            request: self.request,
            attempt: self.attempt,
            content_hash: self.content.fingerprint(),
            stamp: self.stamp,
            result,
        }
    }
}
impl World {
    /// residentと未完了要求を合わせた予約量。workerを起動する前に全要求の和を検査します。
    pub fn reserved_terrain(&self) -> EngineResult<(u64, u64)> {
        let mut count = self.chunks.len() as u64;
        let mut bytes = self.terrain_bytes() as u64;
        let per_chunk = (self.content.layers.len() as u64)
            .checked_mul((CELL_COUNT * size_of::<CellCore>()) as u64)
            .ok_or_else(|| invalid("chunk予約byte数overflow"))?;
        for (_, request) in self.requests.iter() {
            if matches!(
                request.status,
                RequestStatus::Requested | RequestStatus::Preparing | RequestStatus::Ready
            ) {
                let chunks = request.region.chunk_count();
                count = count
                    .checked_add(chunks)
                    .ok_or_else(|| invalid("chunk予約数overflow"))?;
                bytes = bytes
                    .checked_add(
                        chunks
                            .checked_mul(per_chunk)
                            .ok_or_else(|| invalid("生成予約byte数overflow"))?,
                    )
                    .ok_or_else(|| invalid("生成予約byte数overflow"))?;
            }
        }
        Ok((count, bytes))
    }
    pub(crate) fn validate_generation(&self) -> EngineResult<()> {
        let (count, bytes) = self.reserved_terrain()?;
        if count > u64::from(self.limits.resident_chunks) {
            return Err(EngineError::capacity(
                "residentと生成予約のchunk数",
                count,
                self.limits.resident_chunks.into(),
            ));
        }
        if bytes > self.limits.terrain_bytes as u64 {
            return Err(EngineError::capacity(
                "residentと生成予約のbyte数",
                bytes,
                self.limits.terrain_bytes as u64,
            ));
        }
        for (_, request) in self.requests.iter() {
            request.validate()?;
            if !self.content.accepts_content(request.content_hash) {
                return Err(EngineError::new(
                    ErrorCode::MissingDependency,
                    "生成要求のContent版",
                ));
            }
            if matches!(
                request.status,
                RequestStatus::Requested | RequestStatus::Preparing | RequestStatus::Ready
            ) {
                let coords = request
                    .region
                    .coordinates(self.limits.resident_chunks as usize)?;
                if coords.iter().any(|coord| {
                    !self.bounds.intersects_chunk(*coord) || self.chunks.contains_key(coord)
                }) {
                    return Err(conflict("生成予約と現在地形の競合"));
                }
                if let Some(ready) = &request.ready {
                    if ready.len() != coords.len()
                        || ready
                            .iter()
                            .zip(coords)
                            .any(|(chunk, coord)| chunk.coordinate != coord)
                    {
                        return Err(invalid("生成候補のcoverage"));
                    }
                    for chunk in ready {
                        if chunk.pages.len() != self.content.layers.len() {
                            return Err(invalid("生成候補のlayer数"));
                        }
                        let origin = chunk.coordinate.origin()?;
                        for page in &chunk.pages {
                            if page.cells.len() != CELL_COUNT || page.blocks.is_some() {
                                return Err(invalid("生成候補のpage形式"));
                            }
                            for (index, cell) in page.cells.iter().enumerate() {
                                cell.validate(&self.content)?;
                                let point = CellPoint {
                                    x: origin.x + (index % 64) as i32,
                                    y: origin.y + (index / 64) as i32,
                                };
                                if !self.bounds.contains(point) && *cell != CellCore::AIR {
                                    return Err(invalid("生成候補の境界外cell"));
                                }
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }
    /// 要求状態を確認します。別Worldや削除済みIDはエラーです。
    pub fn request(&self, reference: RequestRef) -> EngineResult<&RegionRequest> {
        if reference.world != self.id {
            return Err(EngineError::new(
                ErrorCode::WrongScope,
                "読込要求の所属World",
            ));
        }
        self.requests.get(reference.id)
    }
    /// 次のtick以前に必須だが未公開の要求。空でなければ外部処理を待機できます。
    pub fn pending_at(&self, tick: u64) -> impl Iterator<Item = (RequestRef, &RegionRequest)> + '_ {
        self.requests
            .iter()
            .filter(move |(_, request)| {
                request.due_tick <= tick
                    && !matches!(
                        request.status,
                        RequestStatus::Published | RequestStatus::Cancelled
                    )
            })
            .map(|(id, request)| (RequestRef { world: self.id, id }, request))
    }
}
impl WorldEdit<'_> {
    /// 生成要求を正規状態へ記録します。必須regionの一部を先に公開しません。
    pub fn request_generation(
        &mut self,
        source: StableKey,
        region: ChunkRegion,
        pipeline: &GenerationPipeline,
        due_tick: u64,
    ) -> EngineResult<RequestRef> {
        self.run(|editor| {
            if due_tick < editor.world.tick {
                return Err(invalid("過去tickへの生成要求"));
            }
            for coord in region.coordinates(editor.world.limits.resident_chunks as usize)? {
                if !editor.world.bounds.intersects_chunk(coord) {
                    return Err(invalid("World境界外への生成要求"));
                }
                if editor.world.chunks.contains_key(&coord)
                    || editor.world.archived.contains_key(&coord)
                {
                    return Err(conflict("生成要求のchunkは既にresident"));
                }
                if editor.world.requests.iter().any(|(_, request)| {
                    !matches!(
                        request.status,
                        RequestStatus::Cancelled | RequestStatus::Failed | RequestStatus::Published
                    ) && request.region.contains(coord)
                }) {
                    return Err(conflict("重複する生成要求"));
                }
            }
            let id = editor.world.requests.insert(RegionRequest {
                source,
                region,
                stamp: GenerationStamp {
                    key: pipeline.key,
                    version: pipeline.version,
                    fingerprint: pipeline.fingerprint,
                },
                due_tick,
                attempt: 0,
                content_hash: editor.world.content.fingerprint(),
                status: RequestStatus::Requested,
                ready: None,
                failure: None,
            })?;
            editor.world.validate_generation()?;
            Ok(RequestRef {
                world: editor.world.id,
                id,
            })
        })
    }
    /// 要求をPreparingへ移し、threadへ渡せる入力の所有権を返します。
    pub fn begin_generation(&mut self, reference: RequestRef) -> EngineResult<GenerationTask> {
        self.run(|editor| {
            let request = editor.world.request(reference)?;
            if request.status != RequestStatus::Requested {
                return Err(conflict("生成開始の要求状態"));
            }
            let attempt = request
                .attempt
                .checked_add(1)
                .ok_or_else(|| invalid("生成試行世代の上限"))?;
            let task = GenerationTask {
                request: reference,
                attempt,
                seed: editor.world.seed,
                bounds: editor.world.bounds,
                content: editor
                    .world
                    .content
                    .generation_source(request.content_hash)
                    .ok_or_else(|| {
                        EngineError::new(
                            ErrorCode::MissingDependency,
                            "保存した生成入力のContent版",
                        )
                    })?,
                coordinates: request
                    .region
                    .coordinates(editor.world.limits.resident_chunks as usize)?,
                stamp: request.stamp,
                byte_limit: editor.world.limits.terrain_bytes,
            };
            let request = editor.world.requests.get_mut(reference.id)?;
            request.attempt = attempt;
            request.status = RequestStatus::Preparing;
            Ok(task)
        })
    }
    /// 保存復帰等で失われたworkerを再開できる状態へ戻します。次のbeginで試行世代を進めます。
    /// 再開前のworkerが遅れて完了しても、新しい試行へ混入しません。
    pub fn restart_generation(&mut self, reference: RequestRef) -> EngineResult<()> {
        self.run(|editor| {
            let request = editor.world.request(reference)?;
            if !matches!(
                request.status,
                RequestStatus::Preparing | RequestStatus::Failed
            ) {
                return Err(conflict("再開対象でない生成要求"));
            }
            let request = editor.world.requests.get_mut(reference.id)?;
            request.status = RequestStatus::Requested;
            request.failure = None;
            request.ready = None;
            Ok(())
        })
    }
    /// 遅いworker結果は世代と状態を照合します。ReadyになってもWorld地形へは反映しません。
    pub fn submit_generation(&mut self, result: GenerationResult) -> EngineResult<()> {
        self.run(|editor| {
            let request = editor.world.request(result.request)?;
            if request.status != RequestStatus::Preparing
                || request.stamp != result.stamp
                || request.content_hash != result.content_hash
                || request.attempt != result.attempt
            {
                return Err(conflict("取消済み・異なる入力の生成結果"));
            }
            let request = editor.world.requests.get_mut(result.request.id)?;
            match result.result {
                Ok(chunks) => {
                    request.ready = Some(chunks);
                    request.status = RequestStatus::Ready;
                }
                Err(error) => {
                    request.failure = Some(error);
                    request.status = RequestStatus::Failed;
                }
            }
            Ok(())
        })
    }
    /// 指定tickでregion全体を公開します。順序の決定はworker完了順から独立しています。
    pub fn publish_generation(&mut self, reference: RequestRef) -> EngineResult<()> {
        self.run(|editor| {
            let request = editor.world.request(reference)?;
            if request.status != RequestStatus::Ready || request.due_tick != editor.world.tick {
                return Err(EngineError::new(
                    ErrorCode::InvalidState,
                    "生成公開の準備またはtickが不一致",
                ));
            }
            let stamp = request.stamp;
            let chunks = editor
                .world
                .requests
                .get_mut(reference.id)?
                .ready
                .take()
                .ok_or_else(|| invalid("Readyの候補なし"))?;
            for candidate in chunks {
                if editor.world.chunks.contains_key(&candidate.coordinate) {
                    return Err(conflict("生成候補の公開前にchunkが変更"));
                }
                editor.world.chunks.insert(
                    candidate.coordinate,
                    Chunk {
                        baseline: Some(super::terrain::copy_slice(&candidate.pages)?),
                        layers: candidate.pages,
                        revision: 0,
                        edited: false,
                        active: false,
                        generation: Some(stamp),
                        epoch: 0,
                    },
                );
            }
            editor.world.requests.get_mut(reference.id)?.status = RequestStatus::Published;
            editor.world.apply_material_remaps()?;
            Ok(())
        })
    }
    /// 正規に取消します。workerを強制停止せず、後着結果を拒否します。
    pub fn cancel_generation(&mut self, reference: RequestRef) -> EngineResult<()> {
        self.run(|editor| {
            let request = editor.world.request(reference)?;
            if request.status == RequestStatus::Published {
                return Err(conflict("公開済み要求は取消不可"));
            }
            let request = editor.world.requests.get_mut(reference.id)?;
            request.status = RequestStatus::Cancelled;
            request.ready = None;
            request.failure = None;
            Ok(())
        })
    }
    /// 終端要求を解放します。世代付きallocatorにより後着結果が新要求へ混入しません。
    pub fn forget_generation(&mut self, reference: RequestRef) -> EngineResult<()> {
        self.run(|editor| {
            if !matches!(
                editor.world.request(reference)?.status,
                RequestStatus::Published | RequestStatus::Cancelled | RequestStatus::Failed
            ) {
                return Err(conflict("処理中の要求は先に取消が必要"));
            }
            editor.world.requests.remove(reference.id)?;
            Ok(())
        })
    }
}
