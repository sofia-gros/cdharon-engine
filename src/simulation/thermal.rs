//! scalar simulationで使う整数熱拡散の参照実装を提供します。
//! 隣接pair間で同量を移し、端点で等量を加減することで総energyを保存します。

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThermalError {
    InvalidDimensions,
    SizeMismatch,
    InvalidConductance,
    AllocationFailed,
    Overflow,
}

/// 同一熱容量として、右および下の隣接cell pairだけにenergyを拡散します。
/// 外周は断熱境界です。各pairの一方から引いた整数値を他方へ同量加算します。
pub fn diffuse_energy(
    width: usize,
    height: usize,
    energy: &[i32],
    numerator: u32,
    denominator: u32,
) -> Result<Vec<i32>, ThermalError> {
    let len = width
        .checked_mul(height)
        .ok_or(ThermalError::InvalidDimensions)?;
    let mut delta = Vec::new();
    delta
        .try_reserve_exact(len)
        .map_err(|_| ThermalError::AllocationFailed)?;
    delta.resize(len, 0_i64);
    let mut next = Vec::new();
    next.try_reserve_exact(len)
        .map_err(|_| ThermalError::AllocationFailed)?;
    next.resize(len, 0);
    diffuse_energy_into(
        width,
        height,
        energy,
        numerator,
        denominator,
        &mut delta,
        &mut next,
    )?;
    Ok(next)
}

pub fn diffuse_energy_into(
    width: usize,
    height: usize,
    energy: &[i32],
    numerator: u32,
    denominator: u32,
    delta: &mut [i64],
    next: &mut [i32],
) -> Result<(), ThermalError> {
    diffuse_energy_masked_into(
        width,
        height,
        energy,
        numerator,
        denominator,
        delta,
        next,
        |_| true,
    )
}

/// 両端がactiveな隣接セル間だけでenergyを拡散します。
/// 凍結セルの値は保持し、active領域との境界を断熱境界として扱います。
/// dimensions、係数、入力、scratch領域、active判定を個別に受け取るのは、
/// 既存のscratch再利用APIと同じ呼び出し規約を保ち、World側で確保済みの
/// 一時領域を毎tick再利用するためです。まとめて束縛すると呼び出しごとの
/// scratch所有権が曖昧になるため、この関数に限って引数数lintを許可します。
#[allow(clippy::too_many_arguments)]
pub fn diffuse_energy_masked_into<F>(
    width: usize,
    height: usize,
    energy: &[i32],
    numerator: u32,
    denominator: u32,
    delta: &mut [i64],
    next: &mut [i32],
    mut is_active: F,
) -> Result<(), ThermalError>
where
    F: FnMut(usize) -> bool,
{
    if width == 0 || height == 0 {
        return Err(ThermalError::InvalidDimensions);
    }
    let len = width
        .checked_mul(height)
        .ok_or(ThermalError::InvalidDimensions)?;
    if energy.len() != len || delta.len() != len || next.len() != len {
        return Err(ThermalError::SizeMismatch);
    }
    if denominator == 0 || numerator > denominator {
        return Err(ThermalError::InvalidConductance);
    }
    delta.fill(0);
    let transfer = |a: i32, b: i32| -> Result<i64, ThermalError> {
        let difference = i128::from(a) - i128::from(b);
        let value = difference * i128::from(numerator) / i128::from(denominator);
        i64::try_from(value).map_err(|_| ThermalError::Overflow)
    };
    for y in 0..height {
        for x in 0..width {
            let index = y * width + x;
            for neighbor in [
                (x + 1 < width).then_some(index + 1),
                (y + 1 < height).then_some(index + width),
            ]
            .into_iter()
            .flatten()
            {
                if !is_active(index) || !is_active(neighbor) {
                    continue;
                }
                let amount = transfer(energy[index], energy[neighbor])?;
                delta[index] = delta[index]
                    .checked_sub(amount)
                    .ok_or(ThermalError::Overflow)?;
                delta[neighbor] = delta[neighbor]
                    .checked_add(amount)
                    .ok_or(ThermalError::Overflow)?;
            }
        }
    }
    for (index, (value, change)) in energy.iter().zip(delta.iter()).enumerate() {
        if !is_active(index) {
            next[index] = *value;
            continue;
        }
        let result = i64::from(*value)
            .checked_add(*change)
            .ok_or(ThermalError::Overflow)?;
        next[index] = i32::try_from(result).map_err(|_| ThermalError::Overflow)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diffusion_conserves_energy_and_insulates_outer_edges() {
        let initial = [800, 0, 0, 0, 0, 0];
        let next = diffuse_energy(3, 2, &initial, 1, 8).unwrap();
        assert_eq!(
            initial.iter().map(|v| i64::from(*v)).sum::<i64>(),
            next.iter().map(|v| i64::from(*v)).sum::<i64>()
        );
        assert!(next[1] > 0 && next[3] > 0);
        assert_eq!(diffuse_energy(1, 1, &[42], 1, 8).unwrap(), vec![42]);
    }

    #[test]
    fn inactive_cells_keep_energy_and_block_flux_from_active_neighbors() {
        let energy = [800, 100, 0];
        let mut delta = [0; 3];
        let mut next = [0; 3];
        diffuse_energy_masked_into(3, 1, &energy, 1, 8, &mut delta, &mut next, |index| {
            index != 1
        })
        .unwrap();
        assert_eq!(next, [800, 100, 0]);
    }

    #[test]
    fn bad_input_and_numeric_overflow_are_reported() {
        assert_eq!(
            diffuse_energy(2, 2, &[0], 1, 8),
            Err(ThermalError::SizeMismatch)
        );
        assert_eq!(
            diffuse_energy(1, 1, &[0], 1, 0),
            Err(ThermalError::InvalidConductance)
        );
        assert_eq!(
            diffuse_energy(3, 1, &[i32::MAX, i32::MIN, i32::MAX], 1, 1),
            Err(ThermalError::Overflow)
        );
    }
}
