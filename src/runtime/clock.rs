//! 親の固定tickから進む、保存可能な独立時計。
//!
//! 作物、天候、効果等の時間を同じ速度へ固定しません。分数の余りを保持し、
//! 停止中は時間を発行せず、再開後に壁時計の経過を勝手に取り込みません。

use crate::foundation::{
    EngineError, EngineResult, ErrorCode,
    codec::{Decoder, Encoder, StateCodec},
    transaction::CandidateState,
};

/// 親の1tickにつきnumerator/denominator進む時計です。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClockDomain {
    numerator: u32,
    denominator: u32,
    remainder: u32,
    tick: u64,
    paused: bool,
}
impl ClockDomain {
    /// 正の分母で作ります。numerator=0は意図した停止速度として認めます。
    pub fn new(numerator: u32, denominator: u32) -> EngineResult<Self> {
        if denominator == 0 {
            return Err(EngineError::new(ErrorCode::InvalidValue, "時計の分母"));
        }
        Ok(Self {
            numerator,
            denominator,
            remainder: 0,
            tick: 0,
            paused: false,
        })
    }
    /// 親tick数から発行する子tick数を求め、余りも同時に更新します。
    pub fn advance(&mut self, parent_ticks: u64) -> EngineResult<u64> {
        if self.paused {
            return Ok(0);
        }
        let units =
            u128::from(parent_ticks) * u128::from(self.numerator) + u128::from(self.remainder);
        let issued = u64::try_from(units / u128::from(self.denominator))
            .map_err(|_| EngineError::new(ErrorCode::CapacityExceeded, "子時計の発行数"))?;
        let next = self
            .tick
            .checked_add(issued)
            .ok_or_else(|| EngineError::new(ErrorCode::CapacityExceeded, "子時計の上限"))?;
        self.remainder = (units % u128::from(self.denominator)) as u32;
        self.tick = next;
        Ok(issued)
    }
    /// 停止・再開します。停止前の余りは維持します。
    pub fn pause(&mut self, paused: bool) {
        self.paused = paused;
    }
    /// 発行済みの子tick。
    pub fn tick(&self) -> u64 {
        self.tick
    }
    /// 速度の分子と分母。
    pub fn rate(&self) -> (u32, u32) {
        (self.numerator, self.denominator)
    }
}
impl StateCodec for ClockDomain {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        out.u32(self.numerator)?;
        out.u32(self.denominator)?;
        out.u32(self.remainder)?;
        out.u64(self.tick)?;
        self.paused.encode(out)
    }
    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        let mut clock = Self::new(input.u32()?, input.u32()?)?;
        clock.remainder = input.u32()?;
        clock.tick = input.u64()?;
        clock.paused = bool::decode(input)?;
        if clock.remainder >= clock.denominator {
            return Err(EngineError::new(ErrorCode::InvalidEncoding, "時計の余り"));
        }
        Ok(clock)
    }
}
impl CandidateState for ClockDomain {
    fn try_copy(&self) -> EngineResult<Self> {
        Ok(*self)
    }
    fn validate(&self) -> EngineResult<()> {
        Ok(())
    }
}
