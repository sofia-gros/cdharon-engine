//! 配布先の作業フォルダや区切り文字に依存しない、読み取り専用の資産ファイルシステム。
//!
//! mountは検査済み一覧を一括登録します。大小文字だけが異なる名前、親への参照、
//! Windowsで予約された名前を全OSで拒否し、同じpackが別の意味にならないようにします。

use crate::foundation::{
    EngineError, EngineResult, ErrorCode,
    codec::{Decoder, Encoder},
};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
};
use unicode_normalization::UnicodeNormalization;

/// 仮想パスです。`game:images/player.png`のようにpackageと相対パスを明示します。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct VirtualPath(String);
impl VirtualPath {
    /// 全認定OSで同じ意味になる名前だけを受け付けます。
    pub fn parse(value: &str) -> EngineResult<Self> {
        let normalized: String = value.nfc().collect();
        let value = normalized.as_str();
        let (package, path) = value.split_once(':').ok_or_else(invalid)?;
        crate::foundation::StableKey::resolve(package, "mount")?;
        if value.len() > 1024 || path.is_empty() {
            return Err(invalid());
        }
        for part in path.split('/') {
            let stem = part.split('.').next().unwrap_or("").to_ascii_uppercase();
            if part.is_empty()
                || part == "."
                || part == ".."
                || part.ends_with(['.', ' '])
                || part
                    .chars()
                    .any(|c| c.is_control() || "\\:*?\"<>|".contains(c))
                || ["CON", "PRN", "AUX", "NUL"].contains(&stem.as_str())
                || ((stem.starts_with("COM") || stem.starts_with("LPT"))
                    && stem.len() == 4
                    && matches!(stem.as_bytes()[3], b'1'..=b'9'))
            {
                return Err(invalid());
            }
        }
        Ok(Self(value.into()))
    }
    /// packageを省略した作者向けpathを修飾します。絶対pathや親参照を許可する操作ではありません。
    pub fn resolve(package: &str, path: &str) -> EngineResult<Self> {
        if path.contains(':') {
            Self::parse(path)
        } else {
            Self::parse(&format!("{package}:{path}"))
        }
    }
    /// manifestへ書く、区切り文字を正規化済みの完全名です。
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// packとdirectoryの両方に適用する外部入力の上限です。
#[derive(Clone, Copy, Debug)]
pub struct VfsLimits {
    /// 一つのVFSへ登録できるファイル数。
    pub files: usize,
    /// 一ファイルの圧縮前source byte数。
    pub file_bytes: usize,
    /// pack全体のbyte数。packは圧縮せず、展開bombを持ち込みません。
    pub pack_bytes: usize,
}
impl Default for VfsLimits {
    fn default() -> Self {
        Self {
            files: 65536,
            file_bytes: 64 * 1024 * 1024,
            pack_bytes: 512 * 1024 * 1024,
        }
    }
}

/// SHA-256で各sourceの完全性を検査する、OS非依存の読み取り専用packです。
#[derive(Clone)]
pub struct AssetPack {
    files: BTreeMap<VirtualPath, Arc<[u8]>>,
}
impl AssetPack {
    /// 入力順と無関係に名前順へ正規化します。重複・大小文字衝突はエラーです。
    pub fn new(files: Vec<(VirtualPath, Vec<u8>)>, limits: VfsLimits) -> EngineResult<Self> {
        if files.len() > limits.files {
            return Err(capacity("packファイル数", files.len(), limits.files));
        }
        let mut names = BTreeSet::new();
        let mut total = 0_usize;
        let mut result = BTreeMap::new();
        for (path, bytes) in files {
            if !names.insert(path.0.to_lowercase()) {
                return Err(invalid());
            }
            if bytes.len() > limits.file_bytes {
                return Err(capacity("source byte数", bytes.len(), limits.file_bytes));
            }
            total = total
                .checked_add(bytes.len() + path.0.len() + 40)
                .ok_or_else(invalid)?;
            if total > limits.pack_bytes {
                return Err(capacity("pack byte数", total, limits.pack_bytes));
            }
            result.insert(path, Arc::from(bytes));
        }
        Ok(Self { files: result })
    }
    /// 形式版と長さを含むlittle-endian形式へ保存します。sourceにOS固有pathを含めません。
    pub fn encode(&self, limits: VfsLimits) -> EngineResult<Vec<u8>> {
        if self.files.len() > limits.files
            || self
                .files
                .values()
                .any(|bytes| bytes.len() > limits.file_bytes)
        {
            return Err(invalid());
        }
        let mut out = Encoder::new(limits.pack_bytes);
        out.raw(b"CHPK01")?;
        out.u32(u32::try_from(self.files.len()).map_err(|_| invalid())?)?;
        for (path, bytes) in &self.files {
            out.bytes(path.0.as_bytes())?;
            out.raw(&Sha256::digest(bytes))?;
            out.bytes(bytes)?;
        }
        Ok(out.finish())
    }
    /// checksum・長さ・名前を検査し、すべて成功してからpackを返します。
    pub fn decode(bytes: &[u8], limits: VfsLimits) -> EngineResult<Self> {
        let mut input = Decoder::new(bytes, limits.pack_bytes)?;
        if input.raw(6)? != b"CHPK01" {
            return Err(invalid());
        }
        let count = input.u32()? as usize;
        if count > limits.files || count > input.remaining() / 40 {
            return Err(invalid());
        }
        let mut files = Vec::new();
        files
            .try_reserve(count)
            .map_err(crate::foundation::error::allocation)?;
        for _ in 0..count {
            let path = VirtualPath::parse(
                std::str::from_utf8(input.bytes(1024)?).map_err(|_| invalid())?,
            )?;
            let digest = input.raw(32)?;
            let source = input.bytes(limits.file_bytes)?;
            if Sha256::digest(source).as_slice() != digest {
                return Err(invalid());
            }
            files.push((path, source.to_vec()));
        }
        input.finish()?;
        Self::new(files, limits)
    }
}

#[derive(Clone)]
enum Source {
    Directory { root: Arc<PathBuf>, path: PathBuf },
    Packed(Arc<[u8]>),
}
/// 読み取り専用mount一覧。cloneしたVFSはworkerへ安全に渡せます。
#[derive(Clone)]
pub struct Vfs {
    files: Arc<BTreeMap<VirtualPath, Source>>,
    limits: VfsLimits,
}
impl Vfs {
    /// 空のmount一覧を作ります。存在しない既定フォルダを勝手に作りません。
    pub fn new(limits: VfsLimits) -> Self {
        Self {
            files: Arc::new(BTreeMap::new()),
            limits,
        }
    }
    /// directoryの一覧を検査してmountします。リンクを経由する外部ファイルは登録しません。
    pub fn mount_directory(&mut self, package: &str, root: impl AsRef<Path>) -> EngineResult<()> {
        crate::foundation::StableKey::resolve(package, "mount")?;
        let root = Arc::new(std::fs::canonicalize(root).map_err(io)?);
        if !root.is_dir() {
            return Err(invalid());
        }
        let mut pending = vec![(root.as_ref().clone(), String::new())];
        let mut files = Vec::new();
        let mut visited = 0_usize;
        while let Some((directory, prefix)) = pending.pop() {
            for entry in std::fs::read_dir(directory).map_err(io)? {
                let entry = entry.map_err(io)?;
                visited += 1;
                if visited > self.limits.files {
                    return Err(capacity("directory項目数", visited, self.limits.files));
                }
                let file_type = entry.file_type().map_err(io)?;
                if file_type.is_symlink() {
                    return Err(invalid());
                }
                let name = entry.file_name().into_string().map_err(|_| invalid())?;
                let relative = if prefix.is_empty() {
                    name
                } else {
                    format!("{prefix}/{name}")
                };
                let virtual_path = VirtualPath::parse(&format!("{package}:{relative}"))?;
                let resolved = std::fs::canonicalize(entry.path()).map_err(io)?;
                if !resolved.starts_with(root.as_ref()) {
                    return Err(invalid());
                }
                if file_type.is_dir() {
                    pending.push((resolved, relative));
                } else if file_type.is_file() {
                    files.push((
                        virtual_path,
                        Source::Directory {
                            root: Arc::clone(&root),
                            path: entry.path(),
                        },
                    ));
                } else {
                    return Err(invalid());
                }
            }
        }
        self.insert(files)
    }
    /// 配布packをmountします。既存名の暗黙の上書きを禁止します。
    pub fn mount_pack(&mut self, pack: AssetPack) -> EngineResult<()> {
        self.insert(
            pack.files
                .into_iter()
                .map(|(key, bytes)| (key, Source::Packed(bytes)))
                .collect(),
        )
    }
    fn insert(&mut self, files: Vec<(VirtualPath, Source)>) -> EngineResult<()> {
        if files.len() > self.limits.files.saturating_sub(self.files.len()) {
            return Err(capacity(
                "VFSファイル数",
                self.files.len() + files.len(),
                self.limits.files,
            ));
        }
        let mut names: BTreeSet<_> = self.files.keys().map(|key| key.0.to_lowercase()).collect();
        for (path, _) in &files {
            if !names.insert(path.0.to_lowercase()) {
                return Err(EngineError::new(
                    ErrorCode::DuplicateKey,
                    "VFSの大小文字衝突",
                ));
            }
        }
        Arc::make_mut(&mut self.files).extend(files);
        Ok(())
    }
    /// 上限を超えて読まず、読取直前にもdirectory外への参照を検査します。
    pub fn read(&self, path: &VirtualPath) -> EngineResult<Vec<u8>> {
        match self.files.get(path).ok_or_else(|| {
            EngineError::new(ErrorCode::MissingKey, "VFS資産").detail(path.as_str().to_owned())
        })? {
            Source::Packed(bytes) => Ok(bytes.to_vec()),
            Source::Directory { root, path } => {
                let resolved = std::fs::canonicalize(path).map_err(io)?;
                if !resolved.starts_with(root.as_ref()) {
                    return Err(invalid());
                }
                let mut bytes = Vec::new();
                std::fs::File::open(resolved)
                    .map_err(io)?
                    .take(self.limits.file_bytes as u64 + 1)
                    .read_to_end(&mut bytes)
                    .map_err(io)?;
                if bytes.len() > self.limits.file_bytes {
                    return Err(capacity(
                        "source byte数",
                        bytes.len(),
                        self.limits.file_bytes,
                    ));
                }
                Ok(bytes)
            }
        }
    }
    /// 読み取り可能な正規名を安定順で列挙します。
    pub fn paths(&self) -> impl Iterator<Item = &VirtualPath> {
        self.files.keys()
    }
}
fn invalid() -> EngineError {
    EngineError::new(ErrorCode::InvalidEncoding, "可搬資産pathまたはpack形式")
}
fn io(cause: std::io::Error) -> EngineError {
    EngineError::new(ErrorCode::Io, "資産VFS").detail(cause.to_string())
}
pub(crate) fn capacity(context: &'static str, needed: usize, limit: usize) -> EngineError {
    EngineError::capacity(context, needed as u64, limit as u64)
}
