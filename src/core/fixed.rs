//! Checked signed Q16.16 arithmetic for canonical simulation quantities.

use std::fmt;

const SCALE: i64 = 1 << 16;

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Fixed(i64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FixedError {
    Overflow,
    DivisionByZero,
}

impl fmt::Display for FixedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Overflow => write!(f, "Q16.16 arithmetic overflow"),
            Self::DivisionByZero => write!(f, "Q16.16 division by zero"),
        }
    }
}
impl std::error::Error for FixedError {}

impl Fixed {
    pub const ZERO: Self = Self(0);
    pub const ONE: Self = Self(SCALE);
    pub const SCALE: i64 = SCALE;

    pub const fn from_raw(raw: i64) -> Self {
        Self(raw)
    }
    pub const fn raw(self) -> i64 {
        self.0
    }

    pub fn from_integer(value: i64) -> Result<Self, FixedError> {
        value
            .checked_mul(SCALE)
            .map(Self)
            .ok_or(FixedError::Overflow)
    }

    pub fn from_ratio(numerator: i64, denominator: i64) -> Result<Self, FixedError> {
        if denominator == 0 {
            return Err(FixedError::DivisionByZero);
        }
        let value = i128::from(numerator) * i128::from(SCALE) / i128::from(denominator);
        i64::try_from(value)
            .map(Self)
            .map_err(|_| FixedError::Overflow)
    }

    pub fn checked_add(self, rhs: Self) -> Result<Self, FixedError> {
        self.0
            .checked_add(rhs.0)
            .map(Self)
            .ok_or(FixedError::Overflow)
    }

    pub fn checked_sub(self, rhs: Self) -> Result<Self, FixedError> {
        self.0
            .checked_sub(rhs.0)
            .map(Self)
            .ok_or(FixedError::Overflow)
    }

    pub fn checked_mul(self, rhs: Self) -> Result<Self, FixedError> {
        let value = i128::from(self.0) * i128::from(rhs.0) / i128::from(SCALE);
        i64::try_from(value)
            .map(Self)
            .map_err(|_| FixedError::Overflow)
    }

    pub fn checked_div(self, rhs: Self) -> Result<Self, FixedError> {
        if rhs.0 == 0 {
            return Err(FixedError::DivisionByZero);
        }
        let value = i128::from(self.0) * i128::from(SCALE) / i128::from(rhs.0);
        i64::try_from(value)
            .map(Self)
            .map_err(|_| FixedError::Overflow)
    }

    pub fn floor_i64(self) -> i64 {
        self.0.div_euclid(SCALE)
    }
    pub fn to_f64(self) -> f64 {
        self.0 as f64 / SCALE as f64
    }
}

impl fmt::Display for Fixed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_f64())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn q16_16_uses_checked_128_bit_intermediates_and_zero_toward_zero() {
        let a = Fixed::from_ratio(-1, 3).unwrap();
        assert_eq!(a.raw(), -21_845);
        assert_eq!(
            Fixed::from_ratio(1, 2)
                .unwrap()
                .checked_mul(Fixed::from_integer(3).unwrap())
                .unwrap()
                .raw(),
            98_304
        );
        assert_eq!(Fixed::from_raw(-1).floor_i64(), -1);
        assert_eq!(
            Fixed::from_raw(i64::MAX).checked_add(Fixed::ONE),
            Err(FixedError::Overflow)
        );
        assert_eq!(
            Fixed::ONE.checked_div(Fixed::ZERO),
            Err(FixedError::DivisionByZero)
        );
    }
}
