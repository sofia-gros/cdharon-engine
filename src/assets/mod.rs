//! 型付き資産、scopeの保持権、世代付きcacheとimport worker。
//!
//! `AssetHandle<T>`は軽い参照で、資産の寿命を延ばしません。`AssetLease<T>`はCPU資産を
//! 保持します。Scene loaderはscopeへleaseを登録し、停止時にEngineがまとめて解除します。
//! 差し替え後も旧leaseは旧資産を読み続け、旧handleを新世代へ黙って向け直しません。

pub mod import;
pub mod vfs;
pub use import::{
    Asset, AtlasAsset, AtlasDefinition, AudioAsset, AudioImporter, FontAsset, FontImporter,
    ImageAsset, ImageRect, ImportLimits, Importer, PngImporter,
};
pub use vfs::{AssetPack, Vfs, VfsLimits, VirtualPath};

use crate::{
    foundation::{EngineError, EngineResult, ErrorCode, StableKey},
    runtime::input::ScopeId,
};
use sha2::{Digest, Sha256};
use std::{
    any::{Any, TypeId},
    collections::BTreeMap,
    marker::PhantomData,
    ops::Deref,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc,
    },
    thread,
};

/// 型を含む論理名。実体をまだ読み込んでいない時点でも作れます。
pub struct AssetKey<T: Asset> {
    key: StableKey,
    marker: PhantomData<fn() -> T>,
}
impl<T: Asset> Copy for AssetKey<T> {}
impl<T: Asset> Clone for AssetKey<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T: Asset> AssetKey<T> {
    /// 短い名前はgame packageで修飾します。
    pub fn new(key: &str) -> EngineResult<Self> {
        Ok(Self {
            key: StableKey::resolve("game", key)?,
            marker: PhantomData,
        })
    }
    /// manifestへ使う完全キー。
    pub fn stable_key(self) -> StableKey {
        self.key
    }
}
/// 特定cacheの特定世代を指します。コピーしても保持権は増えません。
pub struct AssetHandle<T: Asset> {
    cache: u64,
    key: StableKey,
    generation: u64,
    marker: PhantomData<fn() -> T>,
}
impl<T: Asset> Copy for AssetHandle<T> {}
impl<T: Asset> Clone for AssetHandle<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T: Asset> std::fmt::Debug for AssetHandle<T> {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("AssetHandle")
            .field("key", &self.key)
            .field("generation", &self.generation)
            .finish()
    }
}
/// import結果を追跡するmanifest。TypeIdやOSの絶対pathは保存しません。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetManifest {
    /// 作者が登録した安定キー。
    pub key: StableKey,
    /// importerの版と設定。
    pub importer: String,
    /// package内のsourceの所在。
    pub source: VirtualPath,
    /// 元sourceのSHA-256。
    pub source_hash: [u8; 32],
    /// 画像等の依存をimportした時点のhash。
    pub dependency_hashes: Vec<[u8; 32]>,
    /// 作者が付けた使用許諾情報。未指定を使用許諾済みとは扱いません。
    pub license: Option<String>,
}
impl AssetManifest {
    /// importer設定・依存・使用許諾を含む正規のSHA-256です。依存先のsourceだけを比較しません。
    pub fn content_hash(&self) -> [u8; 32] {
        let mut hash = Sha256::new();
        for bytes in [
            self.key.as_str().as_bytes(),
            self.importer.as_bytes(),
            self.source.as_str().as_bytes(),
        ] {
            hash.update((bytes.len() as u64).to_le_bytes());
            hash.update(bytes);
        }
        hash.update(self.source_hash);
        hash.update((self.dependency_hashes.len() as u64).to_le_bytes());
        for dependency in &self.dependency_hashes {
            hash.update(dependency);
        }
        hash.update([u8::from(self.license.is_some())]);
        if let Some(license) = &self.license {
            hash.update((license.len() as u64).to_le_bytes());
            hash.update(license.as_bytes());
        }
        hash.finalize().into()
    }
}
struct Budget {
    used: AtomicUsize,
    limit: usize,
}
// 完了待ちの結果にも予算を持たせます。pollされない結果がcache外で無制限に増えません。
struct BudgetPermit {
    bytes: usize,
    budget: Arc<Budget>,
}
impl BudgetPermit {
    fn acquire(budget: &Arc<Budget>, maximum: usize) -> EngineResult<Self> {
        let mut amount = 0;
        budget
            .used
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                amount = maximum.min(budget.limit.saturating_sub(used));
                (amount > 0).then_some(used + amount)
            })
            .map_err(|used| {
                vfs::capacity(
                    "import/cache共有byte数",
                    used.saturating_add(1),
                    budget.limit,
                )
            })?;
        Ok(Self {
            bytes: amount,
            budget: Arc::clone(budget),
        })
    }
    fn shrink(&mut self, bytes: usize) -> EngineResult<()> {
        if bytes > self.bytes {
            return Err(vfs::capacity("展開資産byte数", bytes, self.bytes));
        }
        self.budget
            .used
            .fetch_sub(self.bytes - bytes, Ordering::AcqRel);
        self.bytes = bytes;
        Ok(())
    }
}
impl Drop for BudgetPermit {
    fn drop(&mut self) {
        self.budget.used.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}
struct Resident {
    value: Box<dyn Any + Send + Sync>,
    _permit: BudgetPermit,
    manifest: AssetManifest,
}
/// 特定世代の資産を保持します。別世代へのreloadはこの参照を書き換えません。
pub struct AssetLease<T: Asset> {
    record: Arc<Resident>,
    handle: AssetHandle<T>,
}
impl<T: Asset> Clone for AssetLease<T> {
    fn clone(&self) -> Self {
        Self {
            record: Arc::clone(&self.record),
            handle: self.handle,
        }
    }
}
impl<T: Asset> Deref for AssetLease<T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.record
            .value
            .downcast_ref()
            .expect("型検証済み資産の保持権")
    }
}
impl<T: Asset> AssetLease<T> {
    /// このleaseが保持する世代の軽い参照です。
    pub fn handle(&self) -> AssetHandle<T> {
        self.handle
    }
    /// この世代を作ったimport記録。
    pub fn manifest(&self) -> &AssetManifest {
        &self.record.manifest
    }
}
struct Slot {
    generation: u64,
    kind: TypeId,
    record: Option<Arc<Resident>>,
}
/// cacheとworkerの実行予算です。source側から変更できません。
#[derive(Clone, Copy, Debug)]
pub struct AssetSettings {
    /// 旧世代の外部leaseを含む常駐byte上限。
    pub cache_bytes: usize,
    /// 一度使用したキーの最大数。世代履歴もこの上限内です。
    pub keys: usize,
    /// 同時に実行するdecode worker数。
    pub workers: usize,
    /// worker待ち行列の最大件数。
    pub jobs: usize,
    /// importerへ渡す個別上限。
    pub import: ImportLimits,
}
impl Default for AssetSettings {
    fn default() -> Self {
        Self {
            cache_bytes: 256 * 1024 * 1024,
            keys: 65536,
            workers: 2,
            jobs: 4096,
            import: ImportLimits::default(),
        }
    }
}
type Task = Box<dyn FnOnce() + Send>;
struct Workers {
    sender: Option<mpsc::SyncSender<Task>>,
    threads: Vec<thread::JoinHandle<()>>,
}
impl Workers {
    fn new(workers: usize, jobs: usize) -> EngineResult<Self> {
        let (sender, receiver) = mpsc::sync_channel::<Task>(jobs);
        let receiver = Arc::new(Mutex::new(receiver));
        let mut result = Self {
            sender: Some(sender),
            threads: Vec::new(),
        };
        for index in 0..workers {
            let receiver = Arc::clone(&receiver);
            result.threads.push(
                thread::Builder::new()
                    .name(format!("charon-import-{index}"))
                    .spawn(move || {
                        loop {
                            let task = match receiver.lock() {
                                Ok(receiver) => receiver.recv(),
                                Err(_) => break,
                            };
                            match task {
                                Ok(task) => task(),
                                Err(_) => break,
                            }
                        }
                    })
                    .map_err(|cause| {
                        EngineError::new(ErrorCode::Io, "decode worker作成")
                            .detail(cause.to_string())
                    })?,
            );
        }
        Ok(result)
    }
    fn submit(&self, task: Task) -> EngineResult<()> {
        self.sender
            .as_ref()
            .ok_or_else(|| invalid_state("worker終了済み"))?
            .try_send(task)
            .map_err(|_| EngineError::new(ErrorCode::CapacityExceeded, "import待ち行列"))
    }
}
impl Drop for Workers {
    fn drop(&mut self) {
        self.sender.take();
        for worker in self.threads.drain(..) {
            let _ = worker.join();
        }
    }
}
struct Prepared<T: Asset> {
    permit: BudgetPermit,
    value: T,
    manifest: AssetManifest,
}
// 要求枠はcancelと結果回収のどちらからも、一度だけ返却します。
struct JobPermit(Arc<AtomicUsize>);
impl Drop for JobPermit {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}
/// 非同期importの要求です。取消時は公開権と完了済み結果を回収します。
///
/// Sceneがこの値を保持していても、scope終了時には要求枠と未公開の画像等を回収します。
/// 実行中codecは強制停止せず、終了時に結果を破棄します。結果の公開はAssetServer::poll
/// の呼出時だけ行い、workerの完了順でSceneやWorldを変更しません。
pub struct AssetJob<T: Asset> {
    permit: Arc<Mutex<Option<JobPermit>>>,
    receiver: Arc<Mutex<mpsc::Receiver<EngineResult<Prepared<T>>>>>,
    cancelled: Arc<AtomicBool>,
    scope: ScopeId,
    key: StableKey,
    generation: u64,
    cache: u64,
}
impl<T: Asset> AssetJob<T> {
    /// decode自体が始まっていても、その結果をscope/cacheへ挿入しません。
    ///
    /// 何度呼んでも要求枠は一度だけ返します。完了済み結果はここで破棄し、
    /// この呼出と送信が競合した結果はworker側で回収します。codecの終了待ちは行いません。
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        if let Ok(mut permit) = self.permit.lock() {
            permit.take();
        }
        if let Ok(receiver) = self.receiver.lock() {
            let _ = receiver.try_recv();
        }
    }
}
impl<T: Asset> Drop for AssetJob<T> {
    fn drop(&mut self) {
        self.cancel();
    }
}

// 型を消した回収処理だけをscopeへ渡し、Sceneに残ったjob自体を強く保持しません。
struct ScopeJob {
    flag: std::sync::Weak<AtomicBool>,
    cancel: Box<dyn Fn() + Send + Sync>,
}
/// 全Sceneが共有する資産cache。Worldと異なり正規simulation状態は保持しません。
pub struct AssetServer {
    id: u64,
    vfs: Vfs,
    licenses: BTreeMap<VirtualPath, String>,
    settings: AssetSettings,
    budget: Arc<Budget>,
    slots: BTreeMap<StableKey, Slot>,
    scopes: BTreeMap<ScopeId, Vec<Arc<Resident>>>,
    scope_jobs: BTreeMap<ScopeId, Vec<ScopeJob>>,
    inflight: Arc<AtomicUsize>,
    workers: Workers,
}
impl AssetServer {
    /// VFSは読み取りsnapshotとしてworkerへcloneします。既存のleaseはServerより長く生存できます。
    pub fn new(vfs: Vfs, settings: AssetSettings) -> EngineResult<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        if settings.cache_bytes == 0
            || settings.keys == 0
            || settings.workers == 0
            || settings.workers > 64
            || settings.jobs == 0
        {
            return Err(invalid_state("資産予算"));
        }
        let id = NEXT
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .map_err(|_| invalid_state("cache識別子"))?;
        Ok(Self {
            id,
            vfs,
            licenses: BTreeMap::new(),
            settings,
            budget: Arc::new(Budget {
                used: AtomicUsize::new(0),
                limit: settings.cache_bytes,
            }),
            slots: BTreeMap::new(),
            scopes: BTreeMap::new(),
            scope_jobs: BTreeMap::new(),
            inflight: Arc::new(AtomicUsize::new(0)),
            workers: Workers::new(settings.workers, settings.jobs)?,
        })
    }
    /// mountを起動前に追加するための入口。既存名の上書きはVFSが拒否します。
    pub fn vfs_mut(&mut self) -> &mut Vfs {
        &mut self.vfs
    }
    /// sourceの使用許諾情報を設定します。以後のimport manifestへ複製されます。
    pub fn set_source_license(&mut self, source: &str, license: &str) -> EngineResult<()> {
        if license.is_empty() || license.len() > 4096 {
            return Err(import::invalid("使用許諾情報"));
        }
        let path = VirtualPath::resolve("game", source)?;
        if !self.licenses.contains_key(&path) && self.licenses.len() >= self.settings.keys {
            return Err(invalid_state("使用許諾metadata数"));
        }
        self.licenses.insert(path, license.into());
        Ok(())
    }
    /// Sceneの開始期間を登録します。同じ世代の二重登録は拒否します。
    pub fn open_scope(&mut self, scope: ScopeId) -> EngineResult<()> {
        if self.scopes.contains_key(&scope) {
            return Err(invalid_state("資産scope二重開始"));
        }
        if self.scopes.len() >= self.settings.keys {
            return Err(invalid_state("資産scope上限"));
        }
        self.scopes.insert(scope, Vec::new());
        self.scope_jobs.insert(scope, Vec::new());
        Ok(())
    }
    /// 停止したSceneの全leaseを解放します。workerの遅延結果は以後公開できません。
    pub fn close_scope(&mut self, scope: ScopeId) {
        self.scopes.remove(&scope);
        if let Some(jobs) = self.scope_jobs.remove(&scope) {
            for job in jobs {
                (job.cancel)();
            }
        }
    }
    /// このscopeだけへ保持権を登録するloaderです。
    pub fn loader(&mut self, scope: ScopeId) -> EngineResult<AssetLoader<'_>> {
        if !self.scopes.contains_key(&scope) {
            return Err(invalid_state("停止済み資産scope"));
        }
        Ok(AssetLoader {
            server: self,
            scope,
        })
    }
    /// 型と世代を検査して、独立した保持権を取得します。
    pub fn lease<T: Asset>(&self, handle: AssetHandle<T>) -> EngineResult<AssetLease<T>> {
        if handle.cache != self.id {
            return Err(EngineError::new(ErrorCode::WrongScope, "別cacheの資産"));
        }
        let slot = self.slots.get(&handle.key).ok_or_else(stale)?;
        if slot.generation != handle.generation {
            return Err(stale());
        }
        if slot.kind != TypeId::of::<T>() {
            return Err(EngineError::new(ErrorCode::SchemaMismatch, "資産の型"));
        }
        Ok(AssetLease {
            record: Arc::clone(slot.record.as_ref().ok_or_else(stale)?),
            handle,
        })
    }
    /// logical keyから現在の世代へ解決します。古いhandleを書き換える操作ではありません。
    pub fn resolve<T: Asset>(&self, key: AssetKey<T>) -> EngineResult<AssetHandle<T>> {
        let slot = self.slots.get(&key.key).ok_or_else(stale)?;
        let handle = AssetHandle {
            cache: self.id,
            key: key.key,
            generation: slot.generation,
            marker: PhantomData,
        };
        self.lease(handle)?;
        Ok(handle)
    }
    /// cacheの保持量。cacheから外れた旧世代も、leaseがある間は計上されます。
    pub fn resident_bytes(&self) -> usize {
        self.budget.used.load(Ordering::Acquire)
    }
    /// 誰も保持していない資産を解放します。handleだけでは解放を防げません。
    pub fn collect(&mut self) {
        for slot in self.slots.values_mut() {
            if slot
                .record
                .as_ref()
                .is_some_and(|record| Arc::strong_count(record) == 1)
            {
                slot.record = None;
            }
        }
    }
    fn evict_unused_except(&mut self, key: StableKey) {
        for (current, slot) in &mut self.slots {
            if *current != key
                && slot
                    .record
                    .as_ref()
                    .is_some_and(|record| Arc::strong_count(record) == 1)
            {
                slot.record = None;
            }
        }
    }
    /// 現在cacheへ登録されている世代のmanifestを安定キー順で取得します。
    pub fn manifests(&self) -> Vec<AssetManifest> {
        self.slots
            .values()
            .filter_map(|slot| slot.record.as_ref().map(|record| record.manifest.clone()))
            .collect()
    }
    fn generation(&self, key: StableKey) -> u64 {
        self.slots.get(&key).map_or(0, |slot| slot.generation)
    }
    fn retain<T: Asset>(
        &mut self,
        scope: ScopeId,
        handle: AssetHandle<T>,
    ) -> EngineResult<AssetHandle<T>> {
        let lease = self.lease(handle)?;
        let records = self
            .scopes
            .get_mut(&scope)
            .ok_or_else(|| invalid_state("停止済み資産scope"))?;
        if !records
            .iter()
            .any(|record| Arc::ptr_eq(record, &lease.record))
        {
            if records.len() >= self.settings.keys
                && !records
                    .iter()
                    .any(|record| record.manifest.key == handle.key)
            {
                return Err(invalid_state("scope保持数上限"));
            }
            records
                .try_reserve(1)
                .map_err(crate::foundation::error::allocation)?;
            if let Some(index) = records
                .iter()
                .position(|record| record.manifest.key == handle.key)
            {
                records[index] = lease.record;
            } else {
                records.push(lease.record);
            }
        }
        Ok(handle)
    }
    fn install<T: Asset>(
        &mut self,
        scope: ScopeId,
        prepared: Prepared<T>,
        expected: u64,
    ) -> EngineResult<AssetHandle<T>> {
        if !self.scopes.contains_key(&scope) {
            return Err(invalid_state("停止済み資産scopeへのimport完了"));
        }
        let key = prepared.manifest.key;
        if self.generation(key) != expected {
            return Err(EngineError::new(
                ErrorCode::Conflict,
                "別のimportが先に公開済み",
            ));
        }
        if self
            .slots
            .get(&key)
            .is_some_and(|slot| slot.kind != TypeId::of::<T>())
        {
            return Err(EngineError::new(
                ErrorCode::SchemaMismatch,
                "資産keyの型変更",
            ));
        }
        if !self.slots.contains_key(&key) && self.slots.len() >= self.settings.keys {
            return Err(invalid_state("資産key上限"));
        }
        let generation = expected
            .checked_add(1)
            .ok_or_else(|| invalid_state("資産世代上限"))?;
        let records = self
            .scopes
            .get_mut(&scope)
            .ok_or_else(|| invalid_state("停止済み資産scope"))?;
        if records.len() >= self.settings.keys
            && !records.iter().any(|record| record.manifest.key == key)
        {
            return Err(invalid_state("scope保持数上限"));
        }
        records
            .try_reserve(1)
            .map_err(crate::foundation::error::allocation)?;
        let bytes = prepared.value.resident_bytes();
        if bytes > self.settings.import.decoded_bytes {
            return Err(vfs::capacity(
                "展開資産byte数",
                bytes,
                self.settings.import.decoded_bytes,
            ));
        }
        prepared.value.validate(self.settings.import)?;
        let record = Arc::new(Resident {
            value: Box::new(prepared.value),
            _permit: prepared.permit,
            manifest: prepared.manifest,
        });
        self.slots.insert(
            key,
            Slot {
                generation,
                kind: TypeId::of::<T>(),
                record: Some(record),
            },
        );
        let handle = AssetHandle {
            cache: self.id,
            key,
            generation,
            marker: PhantomData,
        };
        self.retain(scope, handle)
    }
    /// 非同期結果を表示側の明示境界で公開します。未完了ならNone、失敗なら旧世代を残します。
    pub fn poll<T: Asset>(
        &mut self,
        job: &mut AssetJob<T>,
    ) -> EngineResult<Option<AssetHandle<T>>> {
        if job.cache != self.id {
            return Err(EngineError::new(
                ErrorCode::WrongScope,
                "別cacheのimport要求",
            ));
        }
        if job.cancelled.load(Ordering::Acquire) {
            job.cancel();
            return Err(invalid_state("取り消したimport"));
        }
        // lockをmatchの前に解放します。結果処理中のcancelが同じMutexを再取得できます。
        let received = {
            job.receiver
                .lock()
                .map_err(|_| invalid_state("import結果のlock"))?
                .try_recv()
        };
        match received {
            Ok(result) => {
                job.cancel();
                let result = result?;
                if result.manifest.key != job.key {
                    return Err(invalid_state("import要求のkey不一致"));
                }
                self.install(job.scope, result, job.generation).map(Some)
            }
            Err(mpsc::TryRecvError::Empty) => Ok(None),
            Err(_) => {
                job.cancel();
                Err(invalid_state("import workerが終了"))
            }
        }
    }
}
/// Sceneのload、または表示側のreloadで使うscope付き入口です。
pub struct AssetLoader<'a> {
    server: &'a mut AssetServer,
    scope: ScopeId,
}
impl AssetLoader<'_> {
    /// 標準PNG画像。返るhandleの保持権はこのScene scopeが持ちます。
    pub fn image(&mut self, key: &str, source: &str) -> EngineResult<AssetHandle<ImageAsset>> {
        self.load(AssetKey::new(key)?, source, PngImporter::default())
    }
    /// 等間隔sprite sheet。frame寸法は画像pixelであり、block寸法ではありません。
    pub fn spritesheet(
        &mut self,
        key: &str,
        source: &str,
        width: u32,
        height: u32,
    ) -> EngineResult<AssetHandle<ImageAsset>> {
        self.load(
            AssetKey::new(key)?,
            source,
            PngImporter {
                frame_size: Some((width, height)),
            },
        )
    }
    /// WAV/Ogg Vorbisを検証・decodeしてscopeへ保持します。ここでは音を再生しません。
    pub fn audio(&mut self, key: &str, source: &str) -> EngineResult<AssetHandle<AudioAsset>> {
        self.load(AssetKey::new(key)?, source, AudioImporter)
    }
    /// TTF/OTFのfaceを検証して保持します。
    pub fn font(
        &mut self,
        key: &str,
        source: &str,
        face_index: u32,
    ) -> EngineResult<AssetHandle<FontAsset>> {
        self.load(AssetKey::new(key)?, source, FontImporter { face_index })
    }
    /// 独自importerも同じcache・hash・scopeの規則を使います。load中はworker完了を待ちます。
    pub fn load<T: Asset, I: Importer<T>>(
        &mut self,
        key: AssetKey<T>,
        source: &str,
        importer: I,
    ) -> EngineResult<AssetHandle<T>> {
        let path = VirtualPath::resolve("game", source)?;
        if let Some(slot) = self.server.slots.get(&key.key)
            && let Some(record) = &slot.record
        {
            if record.manifest.source != path || record.manifest.importer != importer.identity() {
                return Err(EngineError::new(ErrorCode::Conflict, "同じ資産keyの別定義"));
            }
            let handle = self.server.resolve(key)?;
            return self.server.retain(self.scope, handle);
        }
        let job = self.request(key, source, importer)?;
        let result = job
            .receiver
            .lock()
            .map_err(|_| invalid_state("import結果のlock"))?
            .recv()
            .map_err(|_| invalid_state("import worker完了待ち"))??;
        self.server.install(self.scope, result, job.generation)
    }
    /// reloadを含む非同期要求。完了順は自動的にWorldやSceneへ反映されません。
    pub fn request<T: Asset, I: Importer<T>>(
        &mut self,
        key: AssetKey<T>,
        source: &str,
        importer: I,
    ) -> EngineResult<AssetJob<T>> {
        let path = VirtualPath::resolve("game", source)?;
        let generation = self.server.generation(key.key);
        self.server
            .inflight
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                (used < self.server.settings.jobs).then_some(used + 1)
            })
            .map_err(|used| {
                vfs::capacity("未回収のimport要求数", used + 1, self.server.settings.jobs)
            })?;
        let permit = Arc::new(Mutex::new(Some(JobPermit(Arc::clone(
            &self.server.inflight,
        )))));
        // 不要なcache保持を先に落とし、decode結果と旧世代に共通の上限を使います。
        self.server.evict_unused_except(key.key);
        let budget = Arc::clone(&self.server.budget);
        let vfs = self.server.vfs.clone();
        let limits = self.server.settings.import;
        let license = self.server.licenses.get(&path).cloned();
        let (sender, receiver) = mpsc::sync_channel(1);
        let receiver = Arc::new(Mutex::new(receiver));
        let weak_receiver = Arc::downgrade(&receiver);
        let worker_receiver = weak_receiver.clone();
        let weak_permit = Arc::downgrade(&permit);
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancel = Arc::clone(&cancelled);
        let jobs = self
            .server
            .scope_jobs
            .get_mut(&self.scope)
            .ok_or_else(|| invalid_state("停止済みscope"))?;
        // 作者が取消済みjobを保持し続けても、scope側の回収表は増やし続けません。
        jobs.retain(|job| {
            job.flag
                .upgrade()
                .is_some_and(|flag| !flag.load(Ordering::Acquire))
        });
        jobs.try_reserve(1)
            .map_err(crate::foundation::error::allocation)?;
        let flag = Arc::downgrade(&cancelled);
        let scope_flag = flag.clone();
        jobs.push(ScopeJob {
            flag,
            cancel: Box::new(move || {
                if let Some(flag) = scope_flag.upgrade() {
                    flag.store(true, Ordering::Release);
                }
                if let Some(permit) = weak_permit.upgrade()
                    && let Ok(mut permit) = permit.lock()
                {
                    permit.take();
                }
                if let Some(receiver) = weak_receiver.upgrade()
                    && let Ok(receiver) = receiver.lock()
                {
                    let _ = receiver.try_recv();
                }
            }),
        });
        self.server.workers.submit(Box::new(move || {
            if cancel.load(Ordering::Acquire) {
                return;
            }
            let result = (|| {
                let source = vfs.read(&path)?;
                import::source_limit(&source, limits)?;
                let mut permit = BudgetPermit::acquire(&budget, limits.decoded_bytes)?;
                let bounded = ImportLimits {
                    decoded_bytes: permit.bytes,
                    ..limits
                };
                let value = importer.import(&source, bounded)?;
                value.validate(bounded)?;
                permit.shrink(value.resident_bytes())?;
                Ok(Prepared {
                    permit,
                    value,
                    manifest: AssetManifest {
                        key: key.key,
                        importer: importer.identity(),
                        source: path,
                        source_hash: Sha256::digest(&source).into(),
                        dependency_hashes: importer.dependencies(),
                        license,
                    },
                })
            })();
            if !cancel.load(Ordering::Acquire) {
                let _ = sender.send(result);
            }
            // scope取消とsendが競合し、取消側が空queueを見た場合も、送信側が後から回収します。
            if cancel.load(Ordering::Acquire)
                && let Some(receiver) = worker_receiver.upgrade()
                && let Ok(receiver) = receiver.lock()
            {
                let _ = receiver.try_recv();
            }
        }))?;
        Ok(AssetJob {
            permit,
            receiver,
            cancelled,
            scope: self.scope,
            key: key.key,
            generation,
            cache: self.server.id,
        })
    }
    /// JSON atlasを画像の特定世代へ接続します。範囲外frameは公開前に拒否します。
    pub fn atlas(
        &mut self,
        key: &str,
        source: &str,
        image: AssetHandle<ImageAsset>,
    ) -> EngineResult<AssetHandle<AtlasAsset>> {
        let key = AssetKey::<AtlasAsset>::new(key)?;
        let path = VirtualPath::resolve("game", source)?;
        let image = self.server.lease(image)?;
        let bytes = self.server.vfs.read(&path)?;
        import::source_limit(&bytes, self.server.settings.import)?;
        let definition: AtlasDefinition =
            serde_json::from_slice(&bytes).map_err(import::decode_error)?;
        if definition.schema_version != 1
            || definition.frames.is_empty()
            || definition.frames.len() > self.server.settings.import.frames
        {
            return Err(import::invalid("atlas schemaまたはframe数"));
        }
        for (name, frame) in &definition.frames {
            if name.is_empty() || name.len() > 256 {
                return Err(import::invalid("atlas frame名"));
            }
            frame.validate(image.width, image.height)?;
        }
        let license = self.server.licenses.get(&path).cloned();
        let manifest = AssetManifest {
            key: key.key,
            importer: "charon:atlas-v1".into(),
            source: path,
            source_hash: Sha256::digest(&bytes).into(),
            dependency_hashes: vec![image.manifest().content_hash()],
            license,
        };
        if let Some(slot) = self.server.slots.get(&key.key)
            && let Some(record) = &slot.record
            && record.manifest == manifest
        {
            let handle = self.server.resolve(key)?;
            return self.server.retain(self.scope, handle);
        }
        let value = AtlasAsset {
            image,
            frames: definition.frames,
        };
        let mut permit = BudgetPermit::acquire(&self.server.budget, value.resident_bytes().max(1))?;
        permit.shrink(value.resident_bytes())?;
        self.server.install(
            self.scope,
            Prepared {
                value,
                permit,
                manifest,
            },
            self.server.generation(key.key),
        )
    }
}
fn invalid_state(message: &'static str) -> EngineError {
    EngineError::new(ErrorCode::InvalidState, message)
}
fn stale() -> EngineError {
    EngineError::new(ErrorCode::StaleId, "解放済みまたは旧世代の資産handle")
}
