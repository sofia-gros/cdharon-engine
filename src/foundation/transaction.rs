//! allocator・component・channel等を一緒に変更する候補と公開境界。
//!
//! 型の組合せは作者側で選べます。正規状態を借りたまま独立候補を作り、
//! 操作の成功・不変条件・revision上限をprepareで確認します。
//! Preparedのcommitは新たな検証や確保をせず、候補の所有権を入れ替えます。

use super::codec::{Decoder, Encoder, invalid};
use super::error::{EngineError, EngineResult, ErrorCode};
use super::key::StableKey;

/// 原子的な編集へ参加する作者型の契約です。
///
/// 複製先と元の正規状態が同じ可変bufferを共有してはいけません。
/// 外部資源への送信・音再生等は候補編集で行わず、commit後に処理します。
pub trait CandidateState: Sized {
    /// 独立候補を確保します。失敗した場合は元の値を維持します。
    fn try_copy(&self) -> EngineResult<Self>;
    /// 値域、参照、所有権など、公開に必要な不変条件を検証します。
    fn validate(&self) -> EngineResult<()>;
}

/// 異なる型の正規状態を一つのcommit所有者へまとめます。
pub struct AtomicState<T> {
    scope: StableKey,
    revision: u64,
    state: T,
}

impl<T: CandidateState> AtomicState<T> {
    /// 初期状態を検証して公開します。
    pub fn new(scope: StableKey, state: T) -> EngineResult<Self> {
        state.validate()?;
        Ok(Self {
            scope,
            revision: 0,
            state,
        })
    }
    /// 最後に公開された状態だけを借ります。
    pub fn view(&self) -> &T {
        &self.state
    }
    /// 公開revisionです。候補内の操作回数とは独立しています。
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// 元の状態を可変借用して、並行する別commitを防ぎます。
    pub fn transaction(&mut self) -> EngineResult<AtomicEdit<'_, T>> {
        if self.revision == u64::MAX {
            return Err(EngineError::new(
                ErrorCode::CapacityExceeded,
                "公開revisionの上限",
            ));
        }
        let candidate = self.state.try_copy()?;
        Ok(AtomicEdit {
            target: self,
            candidate,
            failed: None,
        })
    }

    /// 作者の複合codecで公開状態とrevisionを保存します。
    pub fn checkpoint(
        &self,
        byte_limit: usize,
        write: impl FnOnce(&T, &mut Encoder) -> EngineResult<()>,
    ) -> EngineResult<Vec<u8>> {
        use super::codec::StateCodec;
        let mut out = Encoder::new(byte_limit);
        out.raw(b"CHRATM01")?;
        self.scope.encode(&mut out)?;
        out.u64(self.revision)?;
        write(&self.state, &mut out)?;
        Ok(out.finish())
    }

    /// 外部上限で候補を読み、全体の不変条件を検証してから返します。
    pub fn restore(
        bytes: &[u8],
        scope: StableKey,
        byte_limit: usize,
        read: impl FnOnce(&mut Decoder<'_>) -> EngineResult<T>,
    ) -> EngineResult<Self> {
        use super::codec::StateCodec;
        let mut input = Decoder::new(bytes, byte_limit)?;
        if input.raw(8)? != b"CHRATM01" {
            return Err(invalid());
        }
        if StableKey::decode(&mut input)? != scope {
            return Err(EngineError::new(ErrorCode::WrongScope, "複合状態の復元"));
        }
        let revision = input.u64()?;
        let state = read(&mut input)?;
        input.finish()?;
        state.validate()?;
        Ok(Self {
            scope,
            revision,
            state,
        })
    }
}

/// 編集中の候補です。書込の失敗は保持され、無視して公開することはできません。
pub struct AtomicEdit<'a, T> {
    target: &'a mut AtomicState<T>,
    candidate: T,
    failed: Option<EngineError>,
}

impl<'a, T: CandidateState> AtomicEdit<'a, T> {
    /// 候補に対して一つの処理を実行します。返す値に候補への借用は残せません。
    pub fn apply<R>(&mut self, edit: impl FnOnce(&mut T) -> EngineResult<R>) -> EngineResult<R> {
        if let Some(error) = &self.failed {
            return Err(error.clone());
        }
        let result = edit(&mut self.candidate);
        if let Err(error) = &result {
            self.failed = Some(error.clone());
        }
        result
    }
    /// 先行する候補変更を読みます。元の正規状態の代わりとして外へ公開しません。
    pub fn view(&self) -> &T {
        &self.candidate
    }
    /// 全ての検証を完了し、追加処理のないcommit権限へ変換します。
    pub fn prepare(self) -> EngineResult<Prepared<'a, T>> {
        if let Some(error) = self.failed {
            return Err(error);
        }
        self.candidate.validate()?;
        Ok(Prepared {
            target: self.target,
            candidate: self.candidate,
        })
    }
}

/// 検証済み候補の一括公開権限です。破棄した場合も元の状態を維持します。
#[must_use = "commitしなければ候補は公開されません"]
pub struct Prepared<'a, T> {
    target: &'a mut AtomicState<T>,
    candidate: T,
}

impl<T> Prepared<'_, T> {
    /// 所有権を入れ替えて公開revisionを進めます。追加確保やcodecを呼びません。
    ///
    /// 旧状態のDropも作者処理になり得るため、この境界では実行しません。
    /// receiptへ旧状態を渡し、呼出側が公開後の安全な境界で解放します。
    pub fn commit(self) -> CommitReceipt<T> {
        let previous = std::mem::replace(&mut self.target.state, self.candidate);
        self.target.revision += 1;
        CommitReceipt {
            revision: self.target.revision,
            previous,
        }
    }
}

/// 公開後のrevisionと、解放前の旧状態です。
///
/// この値の破棄で旧状態を解放します。作者型のDropによる副作用があっても、
/// 公開済みの状態とrevisionは既に一致しています。
pub struct CommitReceipt<T> {
    /// 新たに公開したrevision。
    pub revision: u64,
    previous: T,
}

impl<T> CommitReceipt<T> {
    /// 旧状態の所有権を取得し、回収queue等へ渡します。正規状態は巻き戻しません。
    pub fn into_previous(self) -> T {
        self.previous
    }
}

macro_rules! scalar_candidate {
    ($($ty:ty),* $(,)?) => {$ (
        impl CandidateState for $ty {
            fn try_copy(&self) -> EngineResult<Self> { Ok(*self) }
            fn validate(&self) -> EngineResult<()> { Ok(()) }
        }
    )*};
}
scalar_candidate!(u8, u16, u32, u64, i32, i64, bool, StableKey);

impl<A: CandidateState, B: CandidateState> CandidateState for (A, B) {
    fn try_copy(&self) -> EngineResult<Self> {
        Ok((self.0.try_copy()?, self.1.try_copy()?))
    }
    fn validate(&self) -> EngineResult<()> {
        self.0.validate()?;
        self.1.validate()
    }
}
