//! 固定シミュレーションtickのための壁時計accumulator。

use std::time::Duration;

const TICKS_PER_SECOND: u128 = 60;
const UNITS_PER_TICK: u128 = 1_000_000_000;

/// 経過時間を整数tickへ変換し、余りを保持する。
///
/// 時間単位は「ナノ秒×60」。1秒を正確に60tickへ分割でき、tick自体は浮動小数点に依存しない。
#[derive(Debug, Default)]
pub struct FixedTickAccumulator {
    accumulated_units: u128,
}

impl FixedTickAccumulator {
    /// 経過時間を加算し、最大`max_ticks`個のtickを取り出す。
    /// 上限を超えた分は捨てず、後続呼び出しで処理する。
    pub fn advance(&mut self, elapsed: Duration, max_ticks: u32) -> u32 {
        self.accumulated_units = self
            .accumulated_units
            .saturating_add(elapsed.as_nanos().saturating_mul(TICKS_PER_SECOND));

        let due = self.accumulated_units / UNITS_PER_TICK;
        let ticks = due.min(max_ticks as u128) as u32;
        self.accumulated_units -= ticks as u128 * UNITS_PER_TICK;
        ticks
    }

    /// 次のtick境界までの壁時計時間。過負荷時は即時再開を返す。
    pub fn until_next_tick(&self) -> Duration {
        let remainder = self.accumulated_units % UNITS_PER_TICK;
        let remaining_units = UNITS_PER_TICK - remainder;
        let nanoseconds = remaining_units.div_ceil(TICKS_PER_SECOND);
        Duration::from_nanos(nanoseconds as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_second_produces_sixty_ticks_without_drift() {
        let mut clock = FixedTickAccumulator::default();
        assert_eq!(clock.advance(Duration::from_secs(1), 60), 60);
        assert_eq!(clock.until_next_tick(), Duration::from_nanos(16_666_667));
    }

    #[test]
    fn tick_budget_retains_backlog() {
        let mut clock = FixedTickAccumulator::default();
        assert_eq!(clock.advance(Duration::from_secs(1), 4), 4);
        assert_eq!(clock.advance(Duration::ZERO, 60), 56);
    }
}
