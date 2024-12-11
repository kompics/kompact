//! Helpers for grouping together data in sub-byte bitfields.
use std::{convert::TryFrom, fmt::Debug};

type Pos = usize;
type Width = usize;

#[derive(Debug, PartialEq)]
/// Errors that can occur when assembling bitfields.
pub enum Error {
    /// The bitfield overflowed into the next primitive.
    Overflow,
    /// The value could not be converted.
    TryFromErr,
}

// TODO  support &[u8]
/// Helper trait for defining where in the [BitFieldSet] to put data.
///
/// # Panics
/// Currently, the `WIDTH` field is used in the internal bit-shifting on bytes, meaning
/// that it will panic if the `WIDTH` > 8.
pub trait BitField<Repr: Sized = u8>: Debug {
    /// The position of the start of this field in the `u8`.
    const POS: Pos;
    /// The width of this field in bits.
    const WIDTH: Width;
}

/// Helpers for storing fields in primitives.
pub trait BitFieldExt {
    /// Store the field at its position in this primitive.
    fn store<Field>(&mut self, field: Field) -> Result<(), Error>
    where
        Field: BitField<u8> + Into<u8>;

    /// Get the field from the position in this primitive.    
    fn get_as<Field>(&self) -> Result<Field, Error>
    where
        Field: BitField<u8> + TryFrom<u8>;

    /// Check if the field would overflow if it were added to this primitive and
    ///  return an `Err` variant if so.    
    fn check_field<Field>(&self) -> Result<(), Error>
    where
        Field: BitField<u8>;
}

impl BitFieldExt for [u8] {
    fn store<Field>(&mut self, field: Field) -> Result<(), Error>
    where
        Field: BitField<u8> + Into<u8>,
    {
        use bit_twiddles::*;

        self.check_field::<Field>()?;

        let pos = Field::POS;
        let width = Field::WIDTH;

        let repr: u8 = field.into();
        (0..width).for_each(|i| {
            let idx = pos + i;
            let (byte, bit) = (idx / 8, idx % 8);
            set_bit_to(&mut self[byte], bit, get_bit(&repr, i));
        });
        Ok(())
    }

    fn get_as<Field>(&self) -> Result<Field, Error>
    where
        Field: BitField<u8> + TryFrom<u8>,
    {
        use bit_twiddles::*;

        self.check_field::<Field>()?;

        let pos = Field::POS;
        let width = Field::WIDTH;

        let mut repr = 0_u8;
        (0..width).for_each(|i| {
            let idx = pos + i;
            let byte = idx / 8;
            let bit = idx % 8;
            set_bit_to(&mut repr, i, get_bit(&self[byte], bit));
        });

        Field::try_from(repr).map_err(|_| Error::TryFromErr)
    }

    fn check_field<Field>(&self) -> Result<(), Error>
    where
        Field: BitField<u8>,
    {
        use bit_twiddles::BITS_PER_BYTE;

        if Field::WIDTH > BITS_PER_BYTE {
            return Err(Error::Overflow);
        }
        let supported_bits = BITS_PER_BYTE * self.len();
        if (Field::POS + Field::WIDTH) > supported_bits {
            Err(Error::Overflow)
        } else {
            Ok(())
        }
    }
}

/// Bit-twiddling helpers
///
/// # Panics
/// All of these will panic if the `pos` parameter exceeds 7
#[allow(unused)]
mod bit_twiddles {
    pub const BITS_PER_BYTE: usize = 8;

    pub fn byte_bit_offset(idx: usize) -> (usize, usize) {
        (idx / BITS_PER_BYTE, idx % BITS_PER_BYTE)
    }

    pub fn get_bit(target: &u8, pos: usize) -> u8 {
        (target >> pos) & 0b1
    }

    pub fn test_bit(target: &u8, pos: usize) -> bool {
        get_bit(target, pos) == 0b01
    }

    pub fn set_bit(target: &mut u8, pos: usize) {
        *target |= 0b1 << pos;
    }

    pub fn set_bit_to(target: &mut u8, pos: usize, val: u8) {
        *target = (*target & !(1 << pos)) | (val << pos);
    }

    pub fn unset_bit(target: &mut u8, pos: usize) {
        *target &= !(0b1 << pos);
    }

    pub fn toggle_bit(target: &mut u8, pos: usize) {
        *target ^= 0b1 << pos;
    }

    #[cfg(test)]
    mod twiddle_tests {
        use super::*;

        #[test]
        fn get() {
            let act: u8 = 0b1010;
            assert_eq!(get_bit(&act, 0), 0b0);
            assert_eq!(get_bit(&act, 1), 0b1);
            assert_eq!(get_bit(&act, 2), 0b0);
            assert_eq!(get_bit(&act, 3), 0b1);
        }

        #[test]
        fn test() {
            let act: u8 = 0b0001;
            assert!(test_bit(&act, 0));
            assert!(!test_bit(&act, 1));
            assert!(!test_bit(&act, 2));
            assert!(!test_bit(&act, 3));
        }

        #[test]
        fn set() {
            let mut act: u8 = 0b0000;
            set_bit(&mut act, 0);
            assert_eq!(act, 0b0001);

            set_bit(&mut act, 1);
            assert_eq!(act, 0b0011);

            set_bit(&mut act, 2);
            assert_eq!(act, 0b0111);

            set_bit(&mut act, 3);
            assert_eq!(act, 0b1111);
        }

        #[test]
        fn set_to() {
            let mut act: u8 = 0b0000;

            set_bit_to(&mut act, 0, 1);
            assert_eq!(act, 0b0001);
            set_bit_to(&mut act, 0, 0);
            assert_eq!(act, 0b0000);

            set_bit_to(&mut act, 1, 1);
            assert_eq!(act, 0b0010);
            set_bit_to(&mut act, 1, 0);
            assert_eq!(act, 0b0000);

            set_bit_to(&mut act, 2, 1);
            assert_eq!(act, 0b0100);
            set_bit_to(&mut act, 2, 0);
            assert_eq!(act, 0b0000);

            set_bit_to(&mut act, 3, 1);
            assert_eq!(act, 0b1000);
            set_bit_to(&mut act, 3, 0);
            assert_eq!(act, 0b0000);
        }

        #[test]
        fn unset() {
            let mut act: u8 = 0b1111;

            unset_bit(&mut act, 0);
            assert_eq!(act, 0b1110);

            unset_bit(&mut act, 1);
            assert_eq!(act, 0b1100);

            unset_bit(&mut act, 2);
            assert_eq!(act, 0b1000);

            unset_bit(&mut act, 3);
            assert_eq!(act, 0b0000);
        }

        #[test]
        fn toggle() {
            let mut act: u8 = 0b1010;

            toggle_bit(&mut act, 0);
            assert_eq!(act, 0b1011);

            toggle_bit(&mut act, 1);
            assert_eq!(act, 0b1001);

            toggle_bit(&mut act, 2);
            assert_eq!(act, 0b1101);

            toggle_bit(&mut act, 3);
            assert_eq!(act, 0b0101);
        }

        const TOO_LARGE_POS: usize = 10;

        #[test]
        #[should_panic]
        fn get_with_large_pos() {
            let act: u8 = 0b1010;
            get_bit(&act, TOO_LARGE_POS);
        }

        #[test]
        #[should_panic]
        fn set_with_large_pos() {
            let mut act: u8 = 0b1010;
            set_bit(&mut act, TOO_LARGE_POS);
        }

        #[test]
        #[should_panic]
        fn set_to_with_large_pos() {
            let mut act: u8 = 0b1010;
            set_bit_to(&mut act, TOO_LARGE_POS, 0b1);
        }

        #[test]
        #[should_panic]
        fn unset_with_large_pos() {
            let mut act: u8 = 0b1010;
            unset_bit(&mut act, TOO_LARGE_POS);
        }

        #[test]
        #[should_panic]
        fn toggle_with_large_pos() {
            let mut act: u8 = 0b1010;
            toggle_bit(&mut act, TOO_LARGE_POS);
        }
    }
}

#[cfg(test)]
#[allow(clippy::useless_vec)]
#[allow(clippy::upper_case_acronyms)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq)]
    #[repr(u8)]
    enum Transport {
        TCP = 0b01,
        UDP = 0b10,
        UDT = 0b11,
    }

    impl BitField for Transport {
        const POS: usize = 0;
        const WIDTH: usize = 2;
    }

    impl From<Transport> for u8 {
        fn from(val: Transport) -> Self {
            val as u8
        }
    }

    impl TryFrom<u8> for Transport {
        type Error = ();

        fn try_from(value: u8) -> Result<Self, Self::Error> {
            match value {
                0b01 => Ok(Transport::TCP),
                0b10 => Ok(Transport::UDP),
                0b11 => Ok(Transport::UDT),
                _ => Err(()),
            }
        }
    }

    #[derive(Debug, PartialEq)]
    #[repr(u8)]
    enum WideWithOffset {
        A = 0b1111_1000,
        B = 0b1111_1100,
        C = 0b1111_1110,
        D = 0b1111_1111,
    }

    impl BitField for WideWithOffset {
        const POS: usize = 7;
        const WIDTH: usize = 2;
    }

    impl From<WideWithOffset> for u8 {
        fn from(val: WideWithOffset) -> Self {
            val as u8
        }
    }

    impl TryFrom<u8> for WideWithOffset {
        type Error = ();

        fn try_from(value: u8) -> Result<Self, Self::Error> {
            match value {
                0b1111_1000 => Ok(WideWithOffset::A),
                0b1111_1100 => Ok(WideWithOffset::B),
                0b1111_1110 => Ok(WideWithOffset::C),
                0b1111_1111 => Ok(WideWithOffset::D),
                _ => Err(()),
            }
        }
    }

    #[derive(Debug, PartialEq)]
    #[repr(u8)]
    enum InvalidWidth {
        DoNotCare = 0b0,
    }

    impl BitField for InvalidWidth {
        const POS: usize = 0;
        const WIDTH: usize = 9; // exceeds current allowed width
    }

    impl From<InvalidWidth> for u8 {
        fn from(val: InvalidWidth) -> Self {
            val as u8
        }
    }

    impl TryFrom<u8> for InvalidWidth {
        type Error = ();

        fn try_from(value: u8) -> Result<Self, Self::Error> {
            match value {
                0b0 => Ok(InvalidWidth::DoNotCare),
                _ => Err(()),
            }
        }
    }

    #[test]
    fn store_and_retrieve() {
        use super::BitFieldExt;
        let mut storage = vec![0u8, 0u8];
        storage.store(Transport::TCP).unwrap();
        assert_eq!(storage.get_as::<Transport>().unwrap(), Transport::TCP);
    }

    #[test]
    fn overwrite() {
        let mut storage = vec![0u8, 0u8];
        storage.store(Transport::TCP).unwrap();
        assert_eq!(storage.get_as::<Transport>().unwrap(), Transport::TCP);
        storage.store(Transport::UDP).unwrap();
        assert_eq!(storage.get_as::<Transport>().unwrap(), Transport::UDP);
    }

    #[test]
    fn store_too_wide() {
        // small_storage is only 8 bits, and thus cannot fit the WideWithOffset
        let mut small_storage = vec![0u8];
        assert_eq!(small_storage.store(WideWithOffset::A), Err(Error::Overflow));
    }

    #[test]
    fn corrupted_read() {
        // 0b0000_0000 is not a valid representation for [Transport]
        let storage = vec![0u8];
        assert_eq!(storage.get_as::<Transport>(), Err(Error::TryFromErr));
    }

    #[test]
    fn invalid_field() {
        let mut storage = vec![0u8, 0u8];
        // `InvalidWidth` has a width of 9, exceeding the current allowed field width
        assert_eq!(storage.store(InvalidWidth::DoNotCare), Err(Error::Overflow));
        assert_eq!(storage.get_as::<InvalidWidth>(), Err(Error::Overflow));
    }
}
