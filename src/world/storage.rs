//! 不変の保存世代と、CURRENTに依存しない復旧。
//!
//! 保存先の実装をGenerationStorageで交換できます。write、sync、世代公開、CURRENT更新、
//! 整理を別操作にし、各境界の失敗でも旧世代を変更しません。入力は全てchecksumと長さを
//! 検証し、World等の型への復元は呼出側の登録codecで行います。

use super::invalid;
use crate::foundation::{
    EngineError, EngineResult, ErrorCode, StableKey,
    codec::{Decoder, Encoder, StateCodec},
    error::allocation,
};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

/// 保存する独立scopeのpayload。scopeの参照辞書等も登録codecのpayloadへ含めます。
pub struct SaveRecord {
    /// World、player、run等の個別rootを識別するキー。
    pub scope: StableKey,
    /// 解釈する必須codecのキー。
    pub kind: StableKey,
    /// codec版。0は使いません。
    pub version: u32,
    /// 明示codecの結果。Rustのメモリdumpではありません。
    pub bytes: Vec<u8>,
}
/// 外部の定義・profileを照合する保存共通情報。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveMetadata {
    /// エンジンの実行profile。
    pub profile: StableKey,
    /// content manifestのBLAKE3。
    pub content_hash: [u8; 32],
    /// 保存した確定tick。
    pub tick: u64,
    /// 必須pluginと版。入力順によらず安定キー順に保存します。
    pub plugins: Vec<(StableKey, u32)>,
}
/// 読込・書込に共通の上限。圧縮サイズだけで判断しません。
#[derive(Clone, Copy, Debug)]
pub struct SaveLimits {
    /// root manifestの上限。
    pub manifest_bytes: usize,
    /// 各payloadの上限。
    pub blob_bytes: usize,
    /// payload合計の上限。
    pub total_bytes: usize,
    /// scopeの数。
    pub records: u32,
    /// 世代一覧を走査する最大数。壊れた世代も含みます。
    pub generations: usize,
}
impl Default for SaveLimits {
    fn default() -> Self {
        Self {
            manifest_bytes: 4 * 1024 * 1024,
            blob_bytes: 256 * 1024 * 1024,
            total_bytes: 512 * 1024 * 1024,
            records: 65536,
            generations: 4096,
        }
    }
}
/// 保存先の世代一覧。一時世代は復旧候補にせず、番号の再利用防止にだけ使います。
#[derive(Clone, Copy, Debug)]
pub struct SaveGeneration {
    /// 単調増加する世代番号。
    pub number: u64,
    /// rootを公開済みか。
    pub published: bool,
}
/// 保存objectを型で区別し、外部入力のpathを保存先へ渡しません。
#[derive(Clone, Copy, Debug)]
pub enum SaveObject {
    /// 最後に書くroot manifest。
    Root,
    /// BLAKE3で識別するpayload。
    Blob([u8; 32]),
}
/// 保存先が実際に提供した同期の強さ。電源断耐性の認定とは別に記録します。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveDurability {
    /// payloadとdirectoryの同期までOSが受理しました。
    FilesAndDirectories,
    /// file同期と世代renameを使用。directory同期の保証は認定していません。
    FilesAndAtomicPublish,
}
/// 独自ファイルシステム・クラウド保存・fault注入も同じ公開手順へ接続します。
pub trait GenerationStorage {
    /// 排他的書込を開始します。失敗時に他のwriterのlockを解除してはいけません。
    fn lock(&mut self) -> EngineResult<()>;
    /// 自身の書込lockだけを解放します。
    fn unlock(&mut self);
    /// 一時世代も含めて上限付きで列挙します。
    fn generations(&mut self, limit: usize) -> EngineResult<Vec<SaveGeneration>>;
    /// 新規の一時世代を作ります。既存世代は上書きしません。
    fn begin(&mut self, generation: u64) -> EngineResult<()>;
    /// 一時世代のobjectを書き、fileをsyncします。
    fn write_staged(
        &mut self,
        generation: u64,
        object: SaveObject,
        bytes: &[u8],
    ) -> EngineResult<()>;
    /// rootを書いた一時世代を同期します。
    fn sync_staged(&mut self, generation: u64) -> EngineResult<SaveDurability>;
    /// 一時世代を同じ保存volume内で公開します。
    fn publish(&mut self, generation: u64) -> EngineResult<()>;
    /// 公開済み世代を上限付きで読みます。
    fn read(&mut self, generation: u64, object: SaveObject, limit: usize) -> EngineResult<Vec<u8>>;
    /// 検証済み世代へCURRENTを置換します。失敗しても世代列挙から復旧できます。
    fn publish_current(&mut self, generation: u64) -> EngineResult<()>;
    /// 自身が管理する古い公開世代だけを削除します。
    fn remove(&mut self, generation: u64) -> EngineResult<()>;
}
/// 保存成功のreceipt。新rootの検証とCURRENT更新が済んだ時だけ返します。
pub struct SaveReceipt {
    /// 公開した世代番号。
    pub generation: u64,
    /// manifest payloadのBLAKE3。
    pub root_hash: [u8; 32],
    /// 保存先が申告した同期方法。
    pub durability: SaveDurability,
    /// 保存自体は成功したが、旧世代整理に失敗した場合の診断。
    pub cleanup_errors: Vec<EngineError>,
}
/// checksumと完全なrecord集合を検証済みの保存bundle。
pub struct LoadedSave {
    /// 採用した公開世代。
    pub generation: u64,
    /// 外部定義を照合する情報。
    pub metadata: SaveMetadata,
    /// 安定scope順のpayload。利用する型での意味検証は復元側が行います。
    pub records: Vec<SaveRecord>,
}
/// 世代保存の共通手順。最低3世代を保持し、旧有効世代をin-placeで更新しません。
pub struct SaveRepository<S: GenerationStorage> {
    storage: S,
    limits: SaveLimits,
    keep: usize,
}
/// 保存manifestに記録した一つの必須payload。
#[derive(Clone, Debug)]
pub struct StoredRecord {
    /// 独立scope。
    pub scope: StableKey,
    /// 必須codec。
    pub kind: StableKey,
    /// codec版。
    pub version: u32,
    /// 展開後長さ。
    pub length: usize,
    /// BLAKE3。
    pub hash: [u8; 32],
}
/// payloadを一度に全保持しない、検証済み保存manifest。
pub struct LoadedManifest {
    /// 保存世代。
    pub generation: u64,
    /// profile、content、tickと必須plugin。
    pub metadata: SaveMetadata,
    /// 全必須payloadの安定scope順一覧。
    pub records: Vec<StoredRecord>,
}
impl<S: GenerationStorage> SaveRepository<S> {
    /// 保存先と容量を指定します。keepを3未満にはできません。
    pub fn new(storage: S, limits: SaveLimits, keep: usize) -> EngineResult<Self> {
        if keep < 3
            || limits.records == 0
            || limits.generations < keep
            || limits.manifest_bytes < 32
        {
            return Err(invalid("保存容量・保持世代数"));
        }
        Ok(Self {
            storage,
            limits,
            keep,
        })
    }
    /// fault注入や計測のため保存先へアクセスします。正規Worldの可変参照は含みません。
    pub fn storage_mut(&mut self) -> &mut S {
        &mut self.storage
    }
    /// 全payloadを検証してから不変世代として公開します。
    pub fn save(
        &mut self,
        metadata: &SaveMetadata,
        records: Vec<SaveRecord>,
    ) -> EngineResult<SaveReceipt> {
        self.save_stream(metadata, records.into_iter().map(Ok))
    }
    /// payloadを一つずつ取得・書込し、rootだけを最後に公開します。
    /// archive全体をRAMへ集めずに保存できます。iteratorの失敗でも旧世代を維持します。
    pub fn save_stream(
        &mut self,
        metadata: &SaveMetadata,
        records: impl IntoIterator<Item = EngineResult<SaveRecord>>,
    ) -> EngineResult<SaveReceipt> {
        self.storage.lock()?;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let generations = self.storage.generations(self.limits.generations)?;
            let next = generations
                .iter()
                .map(|value| value.number)
                .max()
                .unwrap_or(0)
                .checked_add(1)
                .ok_or_else(|| invalid("保存世代の上限"))?;
            self.storage.begin(next)?;
            let mut entries = Vec::new();
            let mut total = 0usize;
            let mut scopes = std::collections::BTreeSet::new();
            for record in records {
                let record = record?;
                if entries.len() >= self.limits.records as usize || !scopes.insert(record.scope) {
                    return Err(invalid("保存record数またはscope重複"));
                }
                if record.version == 0 || record.bytes.len() > self.limits.blob_bytes {
                    return Err(invalid("保存record版またはbyte上限"));
                }
                total = total
                    .checked_add(record.bytes.len())
                    .ok_or_else(|| invalid("保存合計byte数"))?;
                if total > self.limits.total_bytes {
                    return Err(EngineError::capacity(
                        "保存合計byte数",
                        total as u64,
                        self.limits.total_bytes as u64,
                    ));
                }
                let blob = encode_blob(&record, self.limits.blob_bytes)?;
                let hash = *blake3::hash(&blob).as_bytes();
                self.storage
                    .write_staged(next, SaveObject::Blob(hash), &blob)?;
                entries.try_reserve(1).map_err(allocation)?;
                entries.push(StoredRecord {
                    scope: record.scope,
                    kind: record.kind,
                    version: record.version,
                    length: record.bytes.len(),
                    hash,
                });
            }
            entries.sort_by_key(|entry| entry.scope);
            let root = encode_root(metadata, &entries, self.limits)?;
            self.storage.write_staged(next, SaveObject::Root, &root)?;
            let durability = self.storage.sync_staged(next)?;
            self.storage.publish(next)?;
            // renameの成功だけで保存完了とはしません。新しい読取経路で全objectを照合します。
            self.verify_generation(next)?;
            self.storage.publish_current(next)?;
            let mut receipt = SaveReceipt {
                generation: next,
                root_hash: *blake3::hash(&root).as_bytes(),
                durability,
                cleanup_errors: Vec::new(),
            };
            let cleanup = (|| {
                let mut generations = self.storage.generations(self.limits.generations)?;
                generations.retain(|value| value.published);
                generations.sort_by_key(|value| std::cmp::Reverse(value.number));
                let mut valid = 0usize;
                for generation in generations {
                    // 壊れた世代を有効なbackupとして数えません。壊れたものも勝手に削除しません。
                    if self.verify_generation(generation.number).is_ok() {
                        valid += 1;
                        if valid > self.keep {
                            self.storage.remove(generation.number)?;
                        }
                    }
                }
                Ok(())
            })();
            if let Err(error) = cleanup {
                receipt.cleanup_errors.push(error);
            }
            Ok(receipt)
        }));
        self.storage.unlock();
        match result {
            Ok(result) => result,
            Err(panic) => std::panic::resume_unwind(panic),
        }
    }
    /// 世代整理と競合しない読込区間。複数payloadを順に読む間は同じ保存先のwriterを排除します。
    pub fn read_transaction<R>(
        &mut self,
        read: impl FnOnce(&mut Self) -> EngineResult<R>,
    ) -> EngineResult<R> {
        self.storage.lock()?;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| read(self)));
        self.storage.unlock();
        match result {
            Ok(result) => result,
            Err(panic) => std::panic::resume_unwind(panic),
        }
    }
    /// このrepositoryのpayload上限。streamの各bufferを予算内に収めるために使います。
    pub fn limits(&self) -> SaveLimits {
        self.limits
    }
    /// 指定payloadを読んでchecksumを検証します。通常はLoadedManifestのentryを渡します。
    pub fn read_record(
        &mut self,
        generation: u64,
        entry: &StoredRecord,
    ) -> EngineResult<SaveRecord> {
        if entry.length > self.limits.blob_bytes {
            return Err(invalid("保存payload上限"));
        }
        let envelope_limit = entry
            .length
            .checked_add(BLOB_HEADER_MAX)
            .ok_or_else(|| invalid("保存blob上限overflow"))?;
        let envelope =
            self.storage
                .read(generation, SaveObject::Blob(entry.hash), envelope_limit)?;
        if blake3::hash(&envelope).as_bytes() != &entry.hash {
            return Err(invalid("保存blob checksum"));
        }
        let bytes = decode_blob(&envelope, entry, self.limits.blob_bytes)?;
        Ok(SaveRecord {
            scope: entry.scope,
            kind: entry.kind,
            version: entry.version,
            bytes,
        })
    }
    /// 全payloadを一つずつ検証します。返るmanifestはpayloadの所有権を含みません。
    pub fn verify_generation(&mut self, generation: u64) -> EngineResult<LoadedManifest> {
        let root = self
            .storage
            .read(generation, SaveObject::Root, self.limits.manifest_bytes)?;
        let (metadata, records) = decode_root(&root, self.limits)?;
        for record in &records {
            self.read_record(generation, record)?;
        }
        Ok(LoadedManifest {
            generation,
            metadata,
            records,
        })
    }
    /// 参照先まで完全な最終世代を探します。巨大なWorldでもpayloadは一つずつ読みます。
    pub fn latest_manifest(&mut self) -> EngineResult<LoadedManifest> {
        let mut generations = self.storage.generations(self.limits.generations)?;
        generations.retain(|value| value.published);
        generations.sort_by_key(|value| std::cmp::Reverse(value.number));
        let mut last = None;
        for generation in generations {
            match self.verify_generation(generation.number) {
                Ok(manifest) => return Ok(manifest),
                Err(error) => last = Some(error),
            }
        }
        Err(last.unwrap_or_else(|| EngineError::new(ErrorCode::MissingKey, "有効な保存世代なし")))
    }
    /// CURRENTを信用せず、最後の完全な公開rootを探します。
    pub fn load_latest(&mut self) -> EngineResult<LoadedSave> {
        let mut generations = self.storage.generations(self.limits.generations)?;
        generations.retain(|value| value.published);
        generations.sort_by_key(|value| std::cmp::Reverse(value.number));
        let mut last = None;
        for generation in generations {
            match self.load_generation(generation.number) {
                Ok(value) => return Ok(value),
                Err(error) => last = Some(error),
            }
        }
        Err(last.unwrap_or_else(|| EngineError::new(ErrorCode::MissingKey, "有効な保存世代なし")))
    }
    /// 指定世代の形式・長さ・hashを確認します。未知の必須codecを捨てずrecordとして返します。
    pub fn load_generation(&mut self, generation: u64) -> EngineResult<LoadedSave> {
        let root = self
            .storage
            .read(generation, SaveObject::Root, self.limits.manifest_bytes)?;
        let (metadata, entries) = decode_root(&root, self.limits)?;
        let mut records = Vec::new();
        records
            .try_reserve_exact(entries.len())
            .map_err(allocation)?;
        for entry in entries {
            records.push(self.read_record(generation, &entry)?);
        }
        Ok(LoadedSave {
            generation,
            metadata,
            records,
        })
    }
}
const BLOB_HEADER_MAX: usize = 254;
fn encode_blob(record: &SaveRecord, limit: usize) -> EngineResult<Vec<u8>> {
    let limit = limit
        .checked_add(BLOB_HEADER_MAX)
        .ok_or_else(|| invalid("保存blob上限overflow"))?;
    let mut out = Encoder::new(limit);
    out.raw(b"CHBLOB01")?;
    record.kind.encode(&mut out)?;
    out.u16(1)?;
    out.u16(0)?;
    out.u32(record.version)?;
    out.u64(record.bytes.len() as u64)?;
    out.u8(0)?;
    out.raw(blake3::hash(&record.bytes).as_bytes())?;
    out.raw(&record.bytes)?;
    Ok(out.finish())
}
fn decode_blob(bytes: &[u8], entry: &StoredRecord, limit: usize) -> EngineResult<Vec<u8>> {
    let mut input = Decoder::new(
        bytes,
        limit
            .checked_add(BLOB_HEADER_MAX)
            .ok_or_else(|| invalid("保存blob上限overflow"))?,
    )?;
    if input.raw(8)? != b"CHBLOB01"
        || StableKey::decode(&mut input)? != entry.kind
        || input.u16()? != 1
        || input.u16()? != 0
        || input.u32()? != entry.version
        || input.u64()? != entry.length as u64
    {
        return Err(invalid("保存blob header"));
    }
    if input.u8()? != 0 {
        return Err(EngineError::new(
            ErrorCode::UnsupportedCapability,
            "保存blobの圧縮方式",
        ));
    }
    let checksum = input.raw(32)?;
    let payload = input.raw(entry.length)?;
    if blake3::hash(payload).as_bytes() != checksum {
        return Err(invalid("保存payload checksum"));
    }
    input.finish()?;
    super::terrain::copy_slice(payload)
}
fn encode_root(
    metadata: &SaveMetadata,
    records: &[StoredRecord],
    limits: SaveLimits,
) -> EngineResult<Vec<u8>> {
    if records.len() > limits.records as usize
        || records
            .windows(2)
            .any(|pair| pair[0].scope >= pair[1].scope)
    {
        return Err(invalid("保存scopeの重複または上限"));
    }
    let mut out = Encoder::new(limits.manifest_bytes.saturating_sub(32));
    out.raw(b"CHSAVE01")?;
    out.u16(1)?;
    out.u16(0)?;
    out.bytes(env!("CARGO_PKG_VERSION").as_bytes())?;
    metadata.profile.encode(&mut out)?;
    out.raw(&metadata.content_hash)?;
    out.u64(metadata.tick)?;
    let mut plugins = super::terrain::copy_slice(&metadata.plugins)?;
    plugins.sort_by_key(|value| value.0);
    if plugins.windows(2).any(|pair| pair[0].0 == pair[1].0)
        || plugins.iter().any(|value| value.1 == 0)
    {
        return Err(invalid("保存pluginの重複・版"));
    }
    out.u32(u32::try_from(plugins.len()).map_err(|_| invalid("plugin数"))?)?;
    for (key, version) in plugins {
        key.encode(&mut out)?;
        out.u32(version)?;
    }
    out.u32(records.len() as u32)?;
    let mut total = 0usize;
    for record in records {
        if record.version == 0 || record.length > limits.blob_bytes {
            return Err(invalid("保存recordの版またはbyte上限"));
        }
        total = total
            .checked_add(record.length)
            .ok_or_else(|| invalid("保存合計byte数"))?;
        if total > limits.total_bytes {
            return Err(EngineError::capacity(
                "保存合計byte数",
                total as u64,
                limits.total_bytes as u64,
            ));
        }
        record.scope.encode(&mut out)?;
        record.kind.encode(&mut out)?;
        out.u32(record.version)?;
        out.u64(record.length as u64)?;
        out.raw(&record.hash)?;
    }
    let bytes = out.finish();
    let mut out = Encoder::new(limits.manifest_bytes);
    out.raw(&bytes)?;
    out.raw(blake3::hash(&bytes).as_bytes())?;
    Ok(out.finish())
}
fn decode_root(
    bytes: &[u8],
    limits: SaveLimits,
) -> EngineResult<(SaveMetadata, Vec<StoredRecord>)> {
    let split = bytes
        .len()
        .checked_sub(32)
        .ok_or_else(|| invalid("保存root切断"))?;
    if blake3::hash(&bytes[..split]).as_bytes() != &bytes[split..] {
        return Err(invalid("保存root checksum"));
    }
    let mut input = Decoder::new(&bytes[..split], limits.manifest_bytes)?;
    if input.raw(8)? != b"CHSAVE01" || input.u16()? != 1 || input.u16()? != 0 {
        return Err(invalid("保存root形式"));
    }
    let _engine_version =
        std::str::from_utf8(input.bytes(128)?).map_err(|_| invalid("engine版のUTF-8"))?;
    let profile = StableKey::decode(&mut input)?;
    let content_hash = input
        .raw(32)?
        .try_into()
        .map_err(|_| invalid("content hash"))?;
    let tick = input.u64()?;
    let count = input.u32()? as usize;
    if count > input.remaining() / 9 {
        return Err(invalid("保存plugin数"));
    }
    let mut plugins = Vec::new();
    plugins.try_reserve_exact(count).map_err(allocation)?;
    for _ in 0..count {
        let key = StableKey::decode(&mut input)?;
        let version = input.u32()?;
        if version == 0 || plugins.last().is_some_and(|(old, _)| *old >= key) {
            return Err(invalid("保存pluginの順序・版"));
        }
        plugins.push((key, version));
    }
    let count = input.u32()? as usize;
    if count > limits.records as usize || count > input.remaining() / 54 {
        return Err(invalid("保存scope数"));
    }
    let mut records = Vec::new();
    records.try_reserve_exact(count).map_err(allocation)?;
    let mut total = 0usize;
    for _ in 0..count {
        let scope = StableKey::decode(&mut input)?;
        let kind = StableKey::decode(&mut input)?;
        let version = input.u32()?;
        let length = usize::try_from(input.u64()?).map_err(|_| invalid("保存payload長さ"))?;
        let hash = input
            .raw(32)?
            .try_into()
            .map_err(|_| invalid("payload hash"))?;
        total = total
            .checked_add(length)
            .ok_or_else(|| invalid("保存合計長さ"))?;
        if version == 0
            || length > limits.blob_bytes
            || total > limits.total_bytes
            || records
                .last()
                .is_some_and(|old: &StoredRecord| old.scope >= scope)
        {
            return Err(invalid("保存scopeの順序・長さ・版"));
        }
        records.push(StoredRecord {
            scope,
            kind,
            version,
            length,
            hash,
        });
    }
    input.finish()?;
    Ok((
        SaveMetadata {
            profile,
            content_hash,
            tick,
            plugins,
        },
        records,
    ))
}

/// ローカル保存先。rootは構築時にcanonicalizeし、管理する世代以外を削除しません。
pub struct DirectoryStorage {
    root: PathBuf,
    writer: Option<File>,
}
impl DirectoryStorage {
    /// 指定フォルダを保存先として準備します。既存saveを削除しません。
    pub fn new(root: impl AsRef<Path>) -> EngineResult<Self> {
        fs::create_dir_all(root.as_ref()).map_err(io)?;
        let root = fs::canonicalize(root.as_ref()).map_err(io)?;
        Ok(Self { root, writer: None })
    }
    fn require_writer(&self) -> EngineResult<()> {
        if self.writer.is_none() {
            return Err(EngineError::new(
                ErrorCode::InvalidState,
                "保存writer lockが必要",
            ));
        }
        Ok(())
    }
    fn directory(&self, generation: u64, staged: bool) -> PathBuf {
        self.root.join(format!(
            "gen-{generation:020}{}",
            if staged { ".tmp" } else { "" }
        ))
    }
    fn object(object: SaveObject) -> String {
        match object {
            SaveObject::Root => "root.bin".to_owned(),
            SaveObject::Blob(hash) => {
                let mut name = String::with_capacity(69);
                for byte in hash {
                    use std::fmt::Write;
                    write!(&mut name, "{byte:02x}").expect("Stringへの固定長書込");
                }
                name.push_str(".blob");
                name
            }
        }
    }
    fn checked_directory(&self, generation: u64, staged: bool) -> EngineResult<PathBuf> {
        let path = self.directory(generation, staged);
        let metadata = fs::symlink_metadata(&path).map_err(io)?;
        if !metadata.is_dir()
            || metadata.file_type().is_symlink()
            || fs::canonicalize(&path).map_err(io)?.parent() != Some(self.root.as_path())
        {
            return Err(invalid("保存世代がrootの直下でない"));
        }
        Ok(path)
    }
}
impl GenerationStorage for DirectoryStorage {
    fn lock(&mut self) -> EngineResult<()> {
        if self.writer.is_some() {
            return Err(EngineError::new(
                ErrorCode::Conflict,
                "保存writerは既に使用中",
            ));
        }
        let path = self.root.join("LOCK");
        reject_link(&path)?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)
            .map_err(io)?;
        file.try_lock().map_err(|error| {
            EngineError::new(ErrorCode::Conflict, "他の保存writerが使用中")
                .detail(error.to_string())
        })?;
        self.writer = Some(file);
        Ok(())
    }
    fn unlock(&mut self) {
        self.writer.take();
    }
    fn generations(&mut self, limit: usize) -> EngineResult<Vec<SaveGeneration>> {
        let mut values = Vec::new();
        for entry in fs::read_dir(&self.root).map_err(io)? {
            let entry = entry.map_err(io)?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            let Some(number) = name.strip_prefix("gen-") else {
                continue;
            };
            let (number, published) = number
                .strip_suffix(".tmp")
                .map_or((number, true), |value| (value, false));
            if number.len() != 20 || !number.bytes().all(|value| value.is_ascii_digit()) {
                continue;
            }
            let number = number.parse().map_err(|_| invalid("保存世代番号"))?;
            self.checked_directory(number, !published)?;
            if values.len() >= limit {
                return Err(EngineError::capacity(
                    "保存世代一覧数",
                    values.len() as u64 + 1,
                    limit as u64,
                ));
            }
            values.try_reserve(1).map_err(allocation)?;
            values.push(SaveGeneration { number, published });
        }
        Ok(values)
    }
    fn begin(&mut self, generation: u64) -> EngineResult<()> {
        self.require_writer()?;
        fs::create_dir(self.directory(generation, true)).map_err(io)
    }
    fn write_staged(
        &mut self,
        generation: u64,
        object: SaveObject,
        bytes: &[u8],
    ) -> EngineResult<()> {
        self.require_writer()?;
        let path = self
            .checked_directory(generation, true)?
            .join(Self::object(object));
        reject_link(&path)?;
        // 同じhashのpayloadは同一世代内で共有できますが、異なる内容は上書きしません。
        if path.exists() {
            let existing = read_bounded(&path, bytes.len())?;
            if existing == bytes {
                return Ok(());
            }
            return Err(invalid("同じ保存objectの別内容"));
        }
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(path)
            .map_err(io)?;
        file.write_all(bytes).map_err(io)?;
        file.sync_all().map_err(io)
    }
    fn sync_staged(&mut self, generation: u64) -> EngineResult<SaveDurability> {
        self.require_writer()?;
        sync_directory(&self.checked_directory(generation, true)?)
    }
    fn publish(&mut self, generation: u64) -> EngineResult<()> {
        self.require_writer()?;
        let source = self.checked_directory(generation, true)?;
        let target = self.directory(generation, false);
        if target.exists() {
            return Err(EngineError::new(ErrorCode::Conflict, "保存世代は公開済み"));
        }
        fs::rename(source, target).map_err(io)?;
        sync_directory(&self.root)?;
        Ok(())
    }
    fn read(&mut self, generation: u64, object: SaveObject, limit: usize) -> EngineResult<Vec<u8>> {
        read_bounded(
            &self
                .checked_directory(generation, false)?
                .join(Self::object(object)),
            limit,
        )
    }
    fn publish_current(&mut self, generation: u64) -> EngineResult<()> {
        self.require_writer()?;
        let temporary = self.root.join("CURRENT.tmp");
        let target = self.root.join("CURRENT");
        reject_link(&temporary)?;
        reject_link(&target)?;
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temporary)
            .map_err(io)?;
        file.write_all(format!("{generation:020}\n").as_bytes())
            .map_err(io)?;
        file.sync_all().map_err(io)?;
        drop(file);
        fs::rename(temporary, target).map_err(io)?;
        sync_directory(&self.root)?;
        Ok(())
    }
    fn remove(&mut self, generation: u64) -> EngineResult<()> {
        self.require_writer()?;
        let path = self.checked_directory(generation, false)?;
        // computed pathはcanonical rootの直下と確認済みです。リンクを辿った再帰削除はしません。
        for entry in fs::read_dir(&path).map_err(io)? {
            let entry = entry.map_err(io)?;
            let metadata = entry.file_type().map_err(io)?;
            if !metadata.is_file() || metadata.is_symlink() {
                return Err(invalid("保存世代内の未知directory・link"));
            }
        }
        fs::remove_dir_all(path).map_err(io)?;
        sync_directory(&self.root)?;
        Ok(())
    }
}
pub(crate) fn read_bounded(path: &Path, limit: usize) -> EngineResult<Vec<u8>> {
    reject_link(path)?;
    let mut file = File::open(path).map_err(io)?;
    let length = file.metadata().map_err(io)?.len();
    if length > limit as u64 {
        return Err(EngineError::capacity(
            "保存object byte数",
            length,
            limit as u64,
        ));
    }
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(length as usize)
        .map_err(allocation)?;
    // 検査後にfileが伸びてもlimit+1までしか読みません。
    Read::by_ref(&mut file)
        .take((limit as u64).saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(io)?;
    if bytes.len() > limit {
        return Err(EngineError::capacity(
            "保存object byte数",
            bytes.len() as u64,
            limit as u64,
        ));
    }
    Ok(bytes)
}
pub(crate) fn reject_link(path: &Path) -> EngineResult<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(invalid("保存objectのlink")),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io(error)),
    }
}
#[cfg(unix)]
pub(crate) fn sync_directory(path: &Path) -> EngineResult<SaveDurability> {
    File::open(path).map_err(io)?.sync_all().map_err(io)?;
    Ok(SaveDurability::FilesAndDirectories)
}
#[cfg(not(unix))]
pub(crate) fn sync_directory(_path: &Path) -> EngineResult<SaveDurability> {
    // WindowsのFile::sync_allはfile handleに適用済みです。directory fsyncを実行したと
    // 偽らず、保存receiptへ保証の違いを返します。電源断の認定は実機profileで別に行います。
    Ok(SaveDurability::FilesAndAtomicPublish)
}
fn io(error: std::io::Error) -> EngineError {
    EngineError::new(ErrorCode::Io, "世代保存のI/O").detail(error.to_string())
}
