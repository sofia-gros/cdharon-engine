//! 失敗を曖昧な真偽値へ丸めない、基盤共通のエラー。

use std::fmt;

/// 呼出側が文字列を解析せずに失敗を分類するための安定した区分です。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorCode {
    /// 値・範囲・形式が契約に合いません。
    InvalidValue,
    /// 設定した容量を超えます。
    CapacityExceeded,
    /// OS側のメモリ確保に失敗しました。
    AllocationFailed,
    /// 同じ安定キーが登録済みです。
    DuplicateKey,
    /// 必要なキーが登録されていません。
    MissingKey,
    /// 削除済み、または世代が一致しない参照です。
    StaleId,
    /// 所属するWorldまたは保存scopeが異なります。
    WrongScope,
    /// schemaの型・版とデータが一致しません。
    SchemaMismatch,
    /// 候補作成後に元の状態が更新されました。
    Conflict,
    /// 入力データが途中で切れたか、不正な表現です。
    InvalidEncoding,
    /// 現在のlifecycleでは実行できない操作です。
    InvalidState,
    /// 必要な機能または具体的なproviderが存在しません。
    UnsupportedCapability,
    /// 依存する登録が見つかりません。
    MissingDependency,
    /// pluginまたはsystemの依存が循環しています。
    DependencyCycle,
    /// 設定や資産のI/Oに失敗しました。
    Io,
    /// OSのイベントループや表示装置が利用できません。
    DeviceLost,
}

/// 失敗した処理段階です。呼出側の再試行・表示の判断に使います。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorStage {
    /// 作者入力または値域の検証。
    Validation,
    /// 容量の見積もりまたは確保。
    Reservation,
    /// 外部データの読込。
    Decode,
    /// schemaの旧版変換。
    Migration,
    /// 候補を公開する直前の確認。
    Commit,
}

/// 原因、対象、容量の見積もりを保持する日本語の診断です。
///
/// `required/available`は容量失敗時だけ使います。単位は`context`で示し、
/// byte数と個数を混同しないようにします。正規状態には含めません。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EngineError {
    /// 分岐用のエラー区分。
    pub code: ErrorCode,
    /// 対象となる操作またはデータ。
    pub context: &'static str,
    /// 失敗した段階。
    pub stage: ErrorStage,
    /// 判明している対象の安定キー。
    pub target: Option<std::sync::Arc<super::key::StableKey>>,
    /// schema内の対象項目。動的データそのものを診断へ複製しません。
    pub field_path: Option<&'static str>,
    /// parserやOSが返した具体的な原因。確保失敗の診断では省略できます。
    pub detail: Option<std::sync::Arc<str>>,
    /// 呼出側が状況変化後に同じ操作を再試行できるか。
    pub retryable: bool,
    /// 容量不足で必要になった量。
    pub required: Option<u64>,
    /// その操作で使用できる量。
    pub available: Option<u64>,
}

impl EngineError {
    /// 容量以外の失敗を構築します。
    pub const fn new(code: ErrorCode, context: &'static str) -> Self {
        Self {
            code,
            context,
            stage: ErrorStage::Validation,
            target: None,
            field_path: None,
            detail: None,
            retryable: false,
            required: None,
            available: None,
        }
    }

    /// 必要量と上限を含む容量エラーを構築します。
    pub const fn capacity(context: &'static str, required: u64, available: u64) -> Self {
        Self {
            code: ErrorCode::CapacityExceeded,
            context,
            stage: ErrorStage::Reservation,
            target: None,
            field_path: None,
            detail: None,
            retryable: true,
            required: Some(required),
            available: Some(available),
        }
    }

    /// 対象と段階を付与します。新たなメモリ確保を行いません。
    pub fn at(
        mut self,
        stage: ErrorStage,
        target: Option<std::sync::Arc<super::key::StableKey>>,
        field_path: Option<&'static str>,
    ) -> Self {
        self.stage = stage;
        self.target = target;
        self.field_path = field_path;
        self
    }
}

impl fmt::Display for EngineError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {:?}", self.context, self.code)?;
        if let Some(detail) = &self.detail {
            write!(formatter, "（{detail}）")?;
        }
        if let (Some(required), Some(available)) = (self.required, self.available) {
            write!(formatter, "（必要量{required}、利用可能量{available}）")?;
        }
        Ok(())
    }
}

impl std::error::Error for EngineError {}

impl EngineError {
    /// 具体的な原因を付与します。設定全体などの機密値は渡しません。
    pub fn detail(mut self, detail: impl Into<std::sync::Arc<str>>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

/// 基盤の失敗を共通の診断として返す結果型です。
pub type EngineResult<T> = Result<T, EngineError>;

/// 確保失敗を共通の診断へ変換します。
pub(crate) fn allocation(_: std::collections::TryReserveError) -> EngineError {
    EngineError::new(ErrorCode::AllocationFailed, "基盤bufferの確保")
}
