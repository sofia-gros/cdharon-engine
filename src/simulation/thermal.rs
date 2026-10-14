//! Integer, energy-conserving diffusion reference for the scalar simulation path.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThermalError {
    InvalidDimensions,
    SizeMismatch,
    InvalidConductance,
    AllocationFailed,
    Overflow,
}

/// Diffuse equal-capacity cell energy over right/down neighbor pairs.
/// Boundaries are insulated. Every pair transfers equal and opposite integer energy.
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
