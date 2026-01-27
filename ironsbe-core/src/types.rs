//! Primitive type definitions and helpers for SBE encoding.
//!
//! This module provides type definitions that map SBE primitive types
//! to Rust types, along with null value constants and helper functions.

/// SBE primitive type enumeration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PrimitiveType {
    /// Single character (1 byte).
    Char,
    /// Signed 8-bit integer.
    Int8,
    /// Signed 16-bit integer.
    Int16,
    /// Signed 32-bit integer.
    Int32,
    /// Signed 64-bit integer.
    Int64,
    /// Unsigned 8-bit integer.
    Uint8,
    /// Unsigned 16-bit integer.
    Uint16,
    /// Unsigned 32-bit integer.
    Uint32,
    /// Unsigned 64-bit integer.
    Uint64,
    /// 32-bit floating point.
    Float,
    /// 64-bit floating point.
    Double,
}

impl PrimitiveType {
    /// Returns the size of the primitive type in bytes.
    #[must_use]
    pub const fn size(&self) -> usize {
        match self {
            Self::Char | Self::Int8 | Self::Uint8 => 1,
            Self::Int16 | Self::Uint16 => 2,
            Self::Int32 | Self::Uint32 | Self::Float => 4,
            Self::Int64 | Self::Uint64 | Self::Double => 8,
        }
    }

    /// Returns the Rust type name for this primitive.
    #[must_use]
    pub const fn rust_type(&self) -> &'static str {
        match self {
            Self::Char => "u8",
            Self::Int8 => "i8",
            Self::Int16 => "i16",
            Self::Int32 => "i32",
            Self::Int64 => "i64",
            Self::Uint8 => "u8",
            Self::Uint16 => "u16",
            Self::Uint32 => "u32",
            Self::Uint64 => "u64",
            Self::Float => "f32",
            Self::Double => "f64",
        }
    }

    /// Returns the SBE type name.
    #[must_use]
    pub const fn sbe_name(&self) -> &'static str {
        match self {
            Self::Char => "char",
            Self::Int8 => "int8",
            Self::Int16 => "int16",
            Self::Int32 => "int32",
            Self::Int64 => "int64",
            Self::Uint8 => "uint8",
            Self::Uint16 => "uint16",
            Self::Uint32 => "uint32",
            Self::Uint64 => "uint64",
            Self::Float => "float",
            Self::Double => "double",
        }
    }

    /// Parses a primitive type from its SBE name.
    #[must_use]
    pub fn from_sbe_name(name: &str) -> Option<Self> {
        match name {
            "char" => Some(Self::Char),
            "int8" => Some(Self::Int8),
            "int16" => Some(Self::Int16),
            "int32" => Some(Self::Int32),
            "int64" => Some(Self::Int64),
            "uint8" => Some(Self::Uint8),
            "uint16" => Some(Self::Uint16),
            "uint32" => Some(Self::Uint32),
            "uint64" => Some(Self::Uint64),
            "float" => Some(Self::Float),
            "double" => Some(Self::Double),
            _ => None,
        }
    }

    /// Returns true if this is a signed integer type.
    #[must_use]
    pub const fn is_signed(&self) -> bool {
        matches!(self, Self::Int8 | Self::Int16 | Self::Int32 | Self::Int64)
    }

    /// Returns true if this is an unsigned integer type.
    #[must_use]
    pub const fn is_unsigned(&self) -> bool {
        matches!(
            self,
            Self::Uint8 | Self::Uint16 | Self::Uint32 | Self::Uint64
        )
    }

    /// Returns true if this is a floating point type.
    #[must_use]
    pub const fn is_float(&self) -> bool {
        matches!(self, Self::Float | Self::Double)
    }
}

/// Null value constants for SBE primitive types.
pub mod null_values {
    /// Null value for char type.
    pub const CHAR_NULL: u8 = 0;

    /// Null value for int8 type.
    pub const INT8_NULL: i8 = i8::MIN;

    /// Null value for int16 type.
    pub const INT16_NULL: i16 = i16::MIN;

    /// Null value for int32 type.
    pub const INT32_NULL: i32 = i32::MIN;

    /// Null value for int64 type.
    pub const INT64_NULL: i64 = i64::MIN;

    /// Null value for uint8 type.
    pub const UINT8_NULL: u8 = u8::MAX;

    /// Null value for uint16 type.
    pub const UINT16_NULL: u16 = u16::MAX;

    /// Null value for uint32 type.
    pub const UINT32_NULL: u32 = u32::MAX;

    /// Null value for uint64 type.
    pub const UINT64_NULL: u64 = u64::MAX;

    /// Null value for float type (NaN).
    pub const FLOAT_NULL: f32 = f32::NAN;

    /// Null value for double type (NaN).
    pub const DOUBLE_NULL: f64 = f64::NAN;
}

/// Decimal type for fixed-point numbers.
///
/// SBE decimals are represented as a mantissa and exponent pair.
/// The actual value is: mantissa * 10^exponent
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Decimal {
    /// The mantissa (significand).
    pub mantissa: i64,
    /// The exponent (power of 10).
    pub exponent: i8,
}

impl Decimal {
    /// Encoded length of a decimal in bytes (mantissa + exponent).
    pub const ENCODED_LENGTH: usize = 9;

    /// Creates a new decimal value.
    ///
    /// # Arguments
    /// * `mantissa` - The mantissa (significand)
    /// * `exponent` - The exponent (power of 10)
    #[must_use]
    pub const fn new(mantissa: i64, exponent: i8) -> Self {
        Self { mantissa, exponent }
    }

    /// Creates a decimal from a floating point value with specified precision.
    ///
    /// # Arguments
    /// * `value` - The floating point value
    /// * `exponent` - The desired exponent (negative for decimal places)
    #[must_use]
    pub fn from_f64(value: f64, exponent: i8) -> Self {
        let multiplier = 10f64.powi(-exponent as i32);
        let mantissa = (value * multiplier).round() as i64;
        Self { mantissa, exponent }
    }

    /// Converts the decimal to a floating point value.
    #[must_use]
    pub fn to_f64(&self) -> f64 {
        self.mantissa as f64 * 10f64.powi(self.exponent as i32)
    }

    /// Returns true if this decimal represents a null value.
    #[must_use]
    pub const fn is_null(&self) -> bool {
        self.mantissa == i64::MIN
    }

    /// Creates a null decimal value.
    #[must_use]
    pub const fn null() -> Self {
        Self {
            mantissa: i64::MIN,
            exponent: 0,
        }
    }
}

impl std::fmt::Display for Decimal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.is_null() {
            write!(f, "NULL")
        } else {
            write!(f, "{}", self.to_f64())
        }
    }
}

/// Timestamp type representing nanoseconds since Unix epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Timestamp(pub u64);

impl Timestamp {
    /// Encoded length of a timestamp in bytes.
    pub const ENCODED_LENGTH: usize = 8;

    /// Null value for timestamp.
    pub const NULL: Self = Self(u64::MAX);

    /// Creates a new timestamp.
    ///
    /// # Arguments
    /// * `nanos` - Nanoseconds since Unix epoch
    #[must_use]
    pub const fn new(nanos: u64) -> Self {
        Self(nanos)
    }

    /// Creates a timestamp from the current time.
    #[must_use]
    pub fn now() -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};
        let duration = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        Self(duration.as_nanos() as u64)
    }

    /// Returns the timestamp value in nanoseconds.
    #[must_use]
    pub const fn as_nanos(&self) -> u64 {
        self.0
    }

    /// Returns the timestamp value in microseconds.
    #[must_use]
    pub const fn as_micros(&self) -> u64 {
        self.0 / 1_000
    }

    /// Returns the timestamp value in milliseconds.
    #[must_use]
    pub const fn as_millis(&self) -> u64 {
        self.0 / 1_000_000
    }

    /// Returns the timestamp value in seconds.
    #[must_use]
    pub const fn as_secs(&self) -> u64 {
        self.0 / 1_000_000_000
    }

    /// Returns true if this is a null timestamp.
    #[must_use]
    pub const fn is_null(&self) -> bool {
        self.0 == u64::MAX
    }
}

impl From<u64> for Timestamp {
    fn from(nanos: u64) -> Self {
        Self(nanos)
    }
}

impl From<Timestamp> for u64 {
    fn from(ts: Timestamp) -> Self {
        ts.0
    }
}

/// Byte order for SBE encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ByteOrder {
    /// Little-endian byte order (default for SBE).
    #[default]
    LittleEndian,
    /// Big-endian byte order.
    BigEndian,
}

impl ByteOrder {
    /// Parses byte order from a string.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "littleendian" | "little-endian" | "le" => Some(Self::LittleEndian),
            "bigendian" | "big-endian" | "be" => Some(Self::BigEndian),
            _ => None,
        }
    }
}

/// Field presence indicator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Presence {
    /// Field is required and must have a value.
    #[default]
    Required,
    /// Field is optional and may be null.
    Optional,
    /// Field has a constant value defined in the schema.
    Constant,
}

impl Presence {
    /// Parses presence from a string.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "required" => Some(Self::Required),
            "optional" => Some(Self::Optional),
            "constant" => Some(Self::Constant),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_primitive_type_size() {
        assert_eq!(PrimitiveType::Char.size(), 1);
        assert_eq!(PrimitiveType::Int8.size(), 1);
        assert_eq!(PrimitiveType::Uint8.size(), 1);
        assert_eq!(PrimitiveType::Int16.size(), 2);
        assert_eq!(PrimitiveType::Uint16.size(), 2);
        assert_eq!(PrimitiveType::Int32.size(), 4);
        assert_eq!(PrimitiveType::Uint32.size(), 4);
        assert_eq!(PrimitiveType::Float.size(), 4);
        assert_eq!(PrimitiveType::Int64.size(), 8);
        assert_eq!(PrimitiveType::Uint64.size(), 8);
        assert_eq!(PrimitiveType::Double.size(), 8);
    }

    #[test]
    fn test_primitive_type_from_sbe_name() {
        assert_eq!(
            PrimitiveType::from_sbe_name("uint64"),
            Some(PrimitiveType::Uint64)
        );
        assert_eq!(
            PrimitiveType::from_sbe_name("double"),
            Some(PrimitiveType::Double)
        );
        assert_eq!(PrimitiveType::from_sbe_name("invalid"), None);
    }

    #[test]
    fn test_decimal_conversion() {
        let dec = Decimal::new(15050, -2);
        assert!((dec.to_f64() - 150.50).abs() < 0.001);

        let dec2 = Decimal::from_f64(150.50, -2);
        assert_eq!(dec2.mantissa, 15050);
        assert_eq!(dec2.exponent, -2);
    }

    #[test]
    fn test_decimal_null() {
        let null = Decimal::null();
        assert!(null.is_null());

        let valid = Decimal::new(100, 0);
        assert!(!valid.is_null());
    }

    #[test]
    fn test_timestamp() {
        let ts = Timestamp::new(1_000_000_000);
        assert_eq!(ts.as_nanos(), 1_000_000_000);
        assert_eq!(ts.as_micros(), 1_000_000);
        assert_eq!(ts.as_millis(), 1_000);
        assert_eq!(ts.as_secs(), 1);

        assert!(!ts.is_null());
        assert!(Timestamp::NULL.is_null());
    }

    #[test]
    fn test_byte_order() {
        assert_eq!(
            ByteOrder::parse("littleEndian"),
            Some(ByteOrder::LittleEndian)
        );
        assert_eq!(ByteOrder::parse("bigEndian"), Some(ByteOrder::BigEndian));
        assert_eq!(ByteOrder::parse("invalid"), None);
    }

    #[test]
    fn test_presence() {
        assert_eq!(Presence::parse("required"), Some(Presence::Required));
        assert_eq!(Presence::parse("optional"), Some(Presence::Optional));
        assert_eq!(Presence::parse("constant"), Some(Presence::Constant));
        assert_eq!(Presence::parse("invalid"), None);
    }
}
