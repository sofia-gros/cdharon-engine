//! 乱数の状態とアルゴリズムを明示する、表示から独立したprovider契約。
//!
//! SplitMix64の定数と演算順は作者公開の参照実装に従います。
//! <https://prng.di.unimi.it/splitmix64.c>

use super::codec::{Decoder, Encoder, StateCodec};
use super::error::{EngineError, EngineResult, ErrorCode};

/// 作者の乱数実装が保存と再開へ参加するための最小契約です。
pub trait RandomSource: StateCodec {
    /// 次の値を取得します。呼出し回数と順序は作者のScheduleProfileへ属します。
    fn next_u64(&mut self) -> u64;

    /// moduloの偏りを除き、`0..upper`から値を取得します。
    fn below(&mut self, upper: u64) -> EngineResult<u64> {
        if upper == 0 {
            return Err(EngineError::new(
                ErrorCode::InvalidValue,
                "乱数の排他的上限",
            ));
        }
        let threshold = upper.wrapping_neg() % upper;
        loop {
            let sample = self.next_u64();
            if sample >= threshold {
                return Ok(sample % upper);
            }
        }
    }
}

/// SplitMix64の状態付きstreamです。暗号や認証には使用しません。
///
/// tuple方式の標準乱数と並存でき、呼出し順を保持した再現処理に使用できます。
/// 保存codecは内部stateのu64一つです。変更時は登録schema版を更新します。
pub struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    /// 指定した内部初期状態からstreamを作ります。
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }
}

impl RandomSource for SplitMix64 {
    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut value = self.state;
        value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        value ^ (value >> 31)
    }
}

impl StateCodec for SplitMix64 {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        out.u64(self.state)
    }
    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        Ok(Self {
            state: input.u64()?,
        })
    }
}

impl super::transaction::CandidateState for SplitMix64 {
    fn try_copy(&self) -> EngineResult<Self> {
        Ok(Self { state: self.state })
    }
    fn validate(&self) -> EngineResult<()> {
        Ok(())
    }
}
