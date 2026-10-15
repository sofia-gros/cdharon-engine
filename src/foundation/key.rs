//! 登録順やRustの型名に依存しない、保存用の安定キー。

use super::codec::{Decoder, Encoder, StateCodec};
use super::error::{EngineError, EngineResult, ErrorCode};
use std::fmt;

/// `package:name`の検証済みキー。比較順はUTF-8 byte順と一致します。
///
/// 各部分はASCII小文字、数字、`_`、`-`の1〜96文字です。
/// 最大193byteを値の中へ保持し、検証・候補の複製で確保を行いません。
/// 未使用末尾はゼロに固定するため、配列の比較順がキーのbyte順になります。
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StableKey {
    bytes: [u8; 193],
    len: u8,
}

impl StableKey {
    /// 完全修飾されたキーを検証します。
    pub fn parse(value: &str) -> EngineResult<Self> {
        let (package, name) = value.split_once(':').ok_or_else(invalid)?;
        if !part(package) || !part(name) {
            return Err(invalid());
        }
        let mut bytes = [0; 193];
        bytes[..value.len()].copy_from_slice(value.as_bytes());
        Ok(Self {
            bytes,
            len: value.len() as u8,
        })
    }

    /// 短いキーを登録元packageで修飾します。完全キーは再修飾しません。
    pub fn resolve(package: &str, name: &str) -> EngineResult<Self> {
        if !part(package) {
            return Err(invalid());
        }
        if name.contains(':') {
            return Self::parse(name);
        }
        if !part(name) {
            return Err(invalid());
        }
        let mut bytes = [0; 193];
        bytes[..package.len()].copy_from_slice(package.as_bytes());
        bytes[package.len()] = b':';
        bytes[package.len() + 1..package.len() + 1 + name.len()].copy_from_slice(name.as_bytes());
        Ok(Self {
            bytes,
            len: (package.len() + 1 + name.len()) as u8,
        })
    }

    /// 永続化に使用する完全キーを返します。
    pub fn as_str(&self) -> &str {
        // 構築経路がASCIIに限定されており、外部から配列を書き換えられません。
        // unsafe変換を使わず、その不変条件を標準のUTF-8検証でも確認します。
        std::str::from_utf8(&self.bytes[..usize::from(self.len)]).expect("検証済み安定キーはASCII")
    }
}

impl fmt::Debug for StableKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), formatter)
    }
}

impl fmt::Display for StableKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl StateCodec for StableKey {
    fn encode(&self, encoder: &mut Encoder) -> EngineResult<()> {
        encoder.bytes(self.as_str().as_bytes())
    }
    fn decode(decoder: &mut Decoder<'_>) -> EngineResult<Self> {
        let value = std::str::from_utf8(decoder.bytes(193)?).map_err(|_| invalid())?;
        Self::parse(value)
    }
}

fn part(value: &str) -> bool {
    (1..=96).contains(&value.len())
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_' || byte == b'-'
        })
}
fn invalid() -> EngineError {
    EngineError::new(
        ErrorCode::InvalidValue,
        "安定キーはpackage:name形式のASCII小文字等で指定",
    )
}
