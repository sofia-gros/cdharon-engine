//! cell、連続World座標、速度、tickを混同しないための単位型。
//!
//! 値の有限性は構築時に検証し、実際のWorld境界・速度上限は選択した
//! World/providerが検証します。ここで特定ゲームの速度上限を固定しません。

use super::codec::{Decoder, Encoder, StateCodec};
use super::error::{EngineError, EngineResult, ErrorCode};
use super::grid::CellPoint;

/// cell単位の連続した位置。画像pixelや画面座標は受け取りません。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorldPoint {
    x: f64,
    y: f64,
}

impl WorldPoint {
    /// 有限な値だけを受け取ります。負の0のbitもそのまま保持します。
    pub fn new(x: f64, y: f64) -> EngineResult<Self> {
        finite_pair(x, y)?;
        Ok(Self { x, y })
    }
    /// cell単位のx座標。
    pub fn x(self) -> f64 {
        self.x
    }
    /// cell単位のy座標。
    pub fn y(self) -> f64 {
        self.y
    }
}

impl StateCodec for WorldPoint {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        self.x.encode(out)?;
        self.y.encode(out)
    }
    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        Self::new(f64::decode(input)?, f64::decode(input)?)
    }
}

/// cell/秒単位の速度。位置と同じ引数型へ暗黙変換しません。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Velocity {
    x: f64,
    y: f64,
}

impl Velocity {
    /// 有限な各軸の速度を指定します。
    pub fn new(x: f64, y: f64) -> EngineResult<Self> {
        finite_pair(x, y)?;
        Ok(Self { x, y })
    }
    /// 水平方向の速度。
    pub fn x(self) -> f64 {
        self.x
    }
    /// 垂直方向の速度。
    pub fn y(self) -> f64 {
        self.y
    }
}

impl StateCodec for Velocity {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        self.x.encode(out)?;
        self.y.encode(out)
    }
    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        Self::new(f64::decode(input)?, f64::decode(input)?)
    }
}

/// 正規時間の位置。表示frame番号とは独立しています。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Tick(pub u64);
/// 正規時間の長さ。秒との変換にはWorldの時間profileが必要です。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct TickSpan(pub u64);

impl Tick {
    /// overflowせず時刻を進めます。失敗しても元の値は変更しません。
    pub fn checked_advance(self, duration: TickSpan) -> EngineResult<Self> {
        self.0
            .checked_add(duration.0)
            .map(Self)
            .ok_or_else(|| EngineError::new(ErrorCode::CapacityExceeded, "tickの上限"))
    }
}

impl StateCodec for Tick {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        out.u64(self.0)
    }
    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        Ok(Self(input.u64()?))
    }
}
impl StateCodec for TickSpan {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        out.u64(self.0)
    }
    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        Ok(Self(input.u64()?))
    }
}

/// 半開矩形の整数cell範囲。max端点はi32::MAX+1も表せます。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellRect {
    min: CellPoint,
    max_x: i64,
    max_y: i64,
}

impl CellRect {
    /// 空でない範囲を検証します。最大端点をi32へ丸めません。
    pub fn new(min: CellPoint, max_x: i64, max_y: i64) -> EngineResult<Self> {
        let end = i64::from(i32::MAX) + 1;
        if max_x <= i64::from(min.x) || max_y <= i64::from(min.y) || max_x > end || max_y > end {
            return Err(EngineError::new(ErrorCode::InvalidValue, "cell矩形の端点"));
        }
        Ok(Self { min, max_x, max_y })
    }
    /// minを含みmaxを含まない判定です。
    pub fn contains(self, point: CellPoint) -> bool {
        point.x >= self.min.x
            && point.y >= self.min.y
            && i64::from(point.x) < self.max_x
            && i64::from(point.y) < self.max_y
    }
    /// 二つの矩形が正の面積で重なるか判定します。
    pub fn intersects(self, other: Self) -> bool {
        i64::from(self.min.x) < other.max_x
            && i64::from(other.min.x) < self.max_x
            && i64::from(self.min.y) < other.max_y
            && i64::from(other.min.y) < self.max_y
    }
    /// cell総数を検証して返します。全i32平面等のu64を超える面積は拒否します。
    pub fn area(self) -> EngineResult<u64> {
        let width = (self.max_x - i64::from(self.min.x)) as u64;
        let height = (self.max_y - i64::from(self.min.y)) as u64;
        width
            .checked_mul(height)
            .ok_or_else(|| EngineError::new(ErrorCode::CapacityExceeded, "cell矩形の面積"))
    }
}

fn finite_pair(x: f64, y: f64) -> EngineResult<()> {
    if x.is_finite() && y.is_finite() {
        Ok(())
    } else {
        Err(EngineError::new(
            ErrorCode::InvalidValue,
            "座標または速度の非有限値",
        ))
    }
}

impl StateCodec for CellRect {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        self.min.encode(out)?;
        out.i64(self.max_x)?;
        out.i64(self.max_y)
    }
    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        Self::new(CellPoint::decode(input)?, input.i64()?, input.i64()?)
    }
}

macro_rules! unit_candidate {
    ($($ty:ty),* $(,)?) => {$ (
        impl super::transaction::CandidateState for $ty {
            fn try_copy(&self) -> EngineResult<Self> { Ok(*self) }
            // 不変条件は構築・decode時に検証され、内部fieldへの書込は公開しません。
            fn validate(&self) -> EngineResult<()> { Ok(()) }
        }
    )*};
}
unit_candidate!(WorldPoint, Velocity, Tick, TickSpan, CellRect);
