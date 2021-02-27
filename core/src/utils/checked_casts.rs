use std::fmt;

const ILLEGAL_VALUE_DESC: &str = "source value was illegal for the target type";
const OUT_OF_RANGE_DESC: &str = "source value was out of range for the target type";

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct ConvertToIntError<N> {
    original: N,
    description: &'static str,
}

impl<N> ConvertToIntError<N> {
    fn illegal_value(original: N) -> Self {
        ConvertToIntError {
            original,
            description: ILLEGAL_VALUE_DESC,
        }
    }

    fn out_of_range(original: N) -> Self {
        ConvertToIntError {
            original,
            description: OUT_OF_RANGE_DESC,
        }
    }
}

impl<N: fmt::Display> fmt::Display for ConvertToIntError<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Conversion of {} was unsuccessful; {}",
            self.original, self.description
        )
    }
}

/// Casting safely to integers
///
/// Prefer using [TryFrom](std::convert::TryFrom) where available.
pub trait CheckedIntegerCasts<Target> {
    /// Cast to the nearest target value
    ///
    /// Returns errors if there is no equivalent target value
    /// or the source value is out of range for the target.
    fn as_checked(self) -> Result<Target, ConvertToIntError<Self>>
    where
        Self: Sized;

    /// If the cast involves rounding, round up.
    fn ceil_as_checked(self) -> Result<Target, ConvertToIntError<Self>>
    where
        Self: Sized;

    /// If the cast involves rounding, round down.
    fn floor_as_checked(self) -> Result<Target, ConvertToIntError<Self>>
    where
        Self: Sized;
}

impl CheckedIntegerCasts<usize> for f64 {
    fn as_checked(self) -> Result<usize, ConvertToIntError<Self>> {
        if self.is_nan() || self.is_infinite() {
            return Err(ConvertToIntError::illegal_value(self));
        }
        if (self.is_sign_negative() && self != -0.0f64) || (self > usize::MAX as f64) {
            return Err(ConvertToIntError::out_of_range(self));
        }
        Ok(self as usize)
    }

    fn ceil_as_checked(self) -> Result<usize, ConvertToIntError<Self>>
    where
        Self: Sized,
    {
        self.ceil().as_checked()
    }

    fn floor_as_checked(self) -> Result<usize, ConvertToIntError<Self>>
    where
        Self: Sized,
    {
        self.floor().as_checked()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_f64_casts() {
        assert_eq!(1usize, 1.0f64.as_checked().unwrap());
        assert_eq!(1usize, 0.5f64.ceil_as_checked().unwrap());
        assert_eq!(0usize, 0.5f64.floor_as_checked().unwrap());

        assert_eq!(0usize, 0.0f64.as_checked().unwrap());
        assert_eq!(0usize, (-0.0f64).as_checked().unwrap());

        assert_eq!(usize::MAX, (usize::MAX as f64).as_checked().unwrap());

        assert!((f64::NAN).as_checked().is_err());
        assert!((f64::INFINITY).as_checked().is_err());
        assert!((f64::NEG_INFINITY).as_checked().is_err());
        //assert!((usize::MAX as f64 + 1.0).as_checked().is_err());
        assert!((f64::MAX).as_checked().is_err());
        assert!((-1.0f64).as_checked().is_err());
    }
}
