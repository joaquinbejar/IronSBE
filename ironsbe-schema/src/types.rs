//! Schema type definitions.
//!
//! This module contains the data structures representing SBE schema elements
//! including primitives, composites, enums, and sets.

use std::collections::HashMap;

/// Complete SBE schema definition.
#[derive(Debug, Clone)]
pub struct Schema {
    /// Package name (namespace).
    pub package: String,
    /// Schema identifier.
    pub id: u16,
    /// Schema version.
    pub version: u16,
    /// Semantic version string.
    pub semantic_version: String,
    /// Schema description.
    pub description: Option<String>,
    /// Byte order for encoding.
    pub byte_order: ByteOrder,
    /// Header type name.
    pub header_type: String,
    /// Type definitions.
    pub types: Vec<TypeDef>,
    /// Message definitions.
    pub messages: Vec<super::messages::MessageDef>,
    /// Type lookup map (built during parsing).
    type_map: HashMap<String, usize>,
}

impl Schema {
    /// Creates a new empty schema.
    #[must_use]
    pub fn new(package: String, id: u16, version: u16) -> Self {
        Self {
            package,
            id,
            version,
            semantic_version: String::new(),
            description: None,
            byte_order: ByteOrder::LittleEndian,
            header_type: "messageHeader".to_string(),
            types: Vec::new(),
            messages: Vec::new(),
            type_map: HashMap::new(),
        }
    }

    /// Adds a type definition to the schema.
    pub fn add_type(&mut self, type_def: TypeDef) {
        let name = type_def.name().to_string();
        let index = self.types.len();
        self.types.push(type_def);
        self.type_map.insert(name, index);
    }

    /// Looks up a type by name.
    #[must_use]
    pub fn get_type(&self, name: &str) -> Option<&TypeDef> {
        self.type_map.get(name).map(|&idx| &self.types[idx])
    }

    /// Returns true if a type with the given name exists.
    #[must_use]
    pub fn has_type(&self, name: &str) -> bool {
        self.type_map.contains_key(name)
    }

    /// Builds the type lookup map from the types vector.
    pub fn build_type_map(&mut self) {
        self.type_map.clear();
        for (idx, type_def) in self.types.iter().enumerate() {
            self.type_map.insert(type_def.name().to_string(), idx);
        }
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

/// Type definition variants.
#[derive(Debug, Clone)]
pub enum TypeDef {
    /// Primitive type definition.
    Primitive(PrimitiveDef),
    /// Composite type definition.
    Composite(CompositeDef),
    /// Enum type definition.
    Enum(EnumDef),
    /// Set (bitfield) type definition.
    Set(SetDef),
}

impl TypeDef {
    /// Returns the name of the type.
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::Primitive(p) => &p.name,
            Self::Composite(c) => &c.name,
            Self::Enum(e) => &e.name,
            Self::Set(s) => &s.name,
        }
    }

    /// Returns the encoded size of the type in bytes.
    #[must_use]
    pub fn encoded_length(&self) -> usize {
        match self {
            Self::Primitive(p) => p.encoded_length(),
            Self::Composite(c) => c.encoded_length(),
            Self::Enum(e) => e.encoding_type.size(),
            Self::Set(s) => s.encoding_type.size(),
        }
    }

    /// Returns true if this is a primitive type.
    #[must_use]
    pub const fn is_primitive(&self) -> bool {
        matches!(self, Self::Primitive(_))
    }

    /// Returns true if this is a composite type.
    #[must_use]
    pub const fn is_composite(&self) -> bool {
        matches!(self, Self::Composite(_))
    }

    /// Returns true if this is an enum type.
    #[must_use]
    pub const fn is_enum(&self) -> bool {
        matches!(self, Self::Enum(_))
    }

    /// Returns true if this is a set type.
    #[must_use]
    pub const fn is_set(&self) -> bool {
        matches!(self, Self::Set(_))
    }
}

/// Primitive type definition.
#[derive(Debug, Clone)]
pub struct PrimitiveDef {
    /// Type name.
    pub name: String,
    /// Underlying primitive type.
    pub primitive_type: PrimitiveType,
    /// Array length (None for scalar).
    pub length: Option<usize>,
    /// Null value representation.
    pub null_value: Option<String>,
    /// Minimum valid value.
    pub min_value: Option<String>,
    /// Maximum valid value.
    pub max_value: Option<String>,
    /// Character encoding (for char arrays).
    pub character_encoding: Option<String>,
    /// Semantic type.
    pub semantic_type: Option<String>,
    /// Description.
    pub description: Option<String>,
    /// Constant value (if presence is constant).
    pub constant_value: Option<String>,
}

impl PrimitiveDef {
    /// Creates a new primitive type definition.
    #[must_use]
    pub fn new(name: String, primitive_type: PrimitiveType) -> Self {
        Self {
            name,
            primitive_type,
            length: None,
            null_value: None,
            min_value: None,
            max_value: None,
            character_encoding: None,
            semantic_type: None,
            description: None,
            constant_value: None,
        }
    }

    /// Returns the encoded length in bytes.
    #[must_use]
    pub fn encoded_length(&self) -> usize {
        let base_size = self.primitive_type.size();
        self.length.map_or(base_size, |len| base_size * len)
    }

    /// Returns true if this is an array type.
    #[must_use]
    pub fn is_array(&self) -> bool {
        self.length.is_some() && self.length != Some(1)
    }
}

/// SBE primitive types.
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

/// Composite type definition.
#[derive(Debug, Clone)]
pub struct CompositeDef {
    /// Type name.
    pub name: String,
    /// Fields within the composite.
    pub fields: Vec<CompositeField>,
    /// Description.
    pub description: Option<String>,
    /// Semantic type.
    pub semantic_type: Option<String>,
}

impl CompositeDef {
    /// Creates a new composite type definition.
    #[must_use]
    pub fn new(name: String) -> Self {
        Self {
            name,
            fields: Vec::new(),
            description: None,
            semantic_type: None,
        }
    }

    /// Returns the encoded length in bytes.
    #[must_use]
    pub fn encoded_length(&self) -> usize {
        self.fields.iter().map(|f| f.encoded_length).sum()
    }

    /// Adds a field to the composite.
    pub fn add_field(&mut self, field: CompositeField) {
        self.fields.push(field);
    }
}

/// Field within a composite type.
#[derive(Debug, Clone)]
pub struct CompositeField {
    /// Field name.
    pub name: String,
    /// Type name (primitive or another type).
    pub type_name: String,
    /// Primitive type (if directly a primitive).
    pub primitive_type: Option<PrimitiveType>,
    /// Offset within the composite (optional, calculated if not specified).
    pub offset: Option<usize>,
    /// Encoded length in bytes.
    pub encoded_length: usize,
    /// Semantic type.
    pub semantic_type: Option<String>,
    /// Description.
    pub description: Option<String>,
    /// Constant value.
    pub constant_value: Option<String>,
}

impl CompositeField {
    /// Creates a new composite field.
    #[must_use]
    pub fn new(name: String, type_name: String, encoded_length: usize) -> Self {
        Self {
            name,
            type_name,
            primitive_type: None,
            offset: None,
            encoded_length,
            semantic_type: None,
            description: None,
            constant_value: None,
        }
    }
}

/// Enum type definition.
#[derive(Debug, Clone)]
pub struct EnumDef {
    /// Type name.
    pub name: String,
    /// Underlying encoding type.
    pub encoding_type: PrimitiveType,
    /// Valid values.
    pub valid_values: Vec<EnumValue>,
    /// Null value (for optional enums).
    pub null_value: Option<String>,
    /// Description.
    pub description: Option<String>,
}

impl EnumDef {
    /// Creates a new enum type definition.
    #[must_use]
    pub fn new(name: String, encoding_type: PrimitiveType) -> Self {
        Self {
            name,
            encoding_type,
            valid_values: Vec::new(),
            null_value: None,
            description: None,
        }
    }

    /// Adds a valid value to the enum.
    pub fn add_value(&mut self, value: EnumValue) {
        self.valid_values.push(value);
    }

    /// Looks up a value by name.
    #[must_use]
    pub fn get_value(&self, name: &str) -> Option<&EnumValue> {
        self.valid_values.iter().find(|v| v.name == name)
    }
}

/// Enum valid value.
#[derive(Debug, Clone)]
pub struct EnumValue {
    /// Value name.
    pub name: String,
    /// Encoded value (as string, parsed based on encoding type).
    pub value: String,
    /// Description.
    pub description: Option<String>,
    /// Since version.
    pub since_version: Option<u16>,
    /// Deprecated since version.
    pub deprecated: Option<u16>,
}

impl EnumValue {
    /// Creates a new enum value.
    #[must_use]
    pub fn new(name: String, value: String) -> Self {
        Self {
            name,
            value,
            description: None,
            since_version: None,
            deprecated: None,
        }
    }

    /// Parses the value as a u64.
    #[must_use]
    pub fn as_u64(&self) -> Option<u64> {
        self.value.parse().ok()
    }

    /// Parses the value as an i64.
    #[must_use]
    pub fn as_i64(&self) -> Option<i64> {
        self.value.parse().ok()
    }
}

/// Set (bitfield) type definition.
#[derive(Debug, Clone)]
pub struct SetDef {
    /// Type name.
    pub name: String,
    /// Underlying encoding type.
    pub encoding_type: PrimitiveType,
    /// Bit choices.
    pub choices: Vec<SetChoice>,
    /// Description.
    pub description: Option<String>,
}

impl SetDef {
    /// Creates a new set type definition.
    #[must_use]
    pub fn new(name: String, encoding_type: PrimitiveType) -> Self {
        Self {
            name,
            encoding_type,
            choices: Vec::new(),
            description: None,
        }
    }

    /// Adds a choice to the set.
    pub fn add_choice(&mut self, choice: SetChoice) {
        self.choices.push(choice);
    }

    /// Looks up a choice by name.
    #[must_use]
    pub fn get_choice(&self, name: &str) -> Option<&SetChoice> {
        self.choices.iter().find(|c| c.name == name)
    }
}

/// Set choice (bit position).
#[derive(Debug, Clone)]
pub struct SetChoice {
    /// Choice name.
    pub name: String,
    /// Bit position (0-based).
    pub bit_position: u8,
    /// Description.
    pub description: Option<String>,
    /// Since version.
    pub since_version: Option<u16>,
    /// Deprecated since version.
    pub deprecated: Option<u16>,
}

impl SetChoice {
    /// Creates a new set choice.
    #[must_use]
    pub fn new(name: String, bit_position: u8) -> Self {
        Self {
            name,
            bit_position,
            description: None,
            since_version: None,
            deprecated: None,
        }
    }

    /// Returns the bit mask for this choice.
    #[must_use]
    pub const fn mask(&self) -> u64 {
        1u64 << self.bit_position
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
        assert_eq!(PrimitiveType::Int64.size(), 8);
        assert_eq!(PrimitiveType::Double.size(), 8);
    }

    #[test]
    fn test_primitive_def_encoded_length() {
        let scalar = PrimitiveDef::new("price".to_string(), PrimitiveType::Int64);
        assert_eq!(scalar.encoded_length(), 8);

        let mut array = PrimitiveDef::new("symbol".to_string(), PrimitiveType::Char);
        array.length = Some(8);
        assert_eq!(array.encoded_length(), 8);
    }

    #[test]
    fn test_composite_encoded_length() {
        let mut composite = CompositeDef::new("decimal".to_string());
        composite.add_field(CompositeField::new(
            "mantissa".to_string(),
            "int64".to_string(),
            8,
        ));
        composite.add_field(CompositeField::new(
            "exponent".to_string(),
            "int8".to_string(),
            1,
        ));
        assert_eq!(composite.encoded_length(), 9);
    }

    #[test]
    fn test_enum_def() {
        let mut enum_def = EnumDef::new("Side".to_string(), PrimitiveType::Uint8);
        enum_def.add_value(EnumValue::new("Buy".to_string(), "1".to_string()));
        enum_def.add_value(EnumValue::new("Sell".to_string(), "2".to_string()));

        assert_eq!(enum_def.valid_values.len(), 2);
        assert_eq!(enum_def.get_value("Buy").unwrap().as_u64(), Some(1));
    }

    #[test]
    fn test_set_choice_mask() {
        let choice = SetChoice::new("Flag1".to_string(), 0);
        assert_eq!(choice.mask(), 1);

        let choice2 = SetChoice::new("Flag8".to_string(), 7);
        assert_eq!(choice2.mask(), 128);
    }

    #[test]
    fn test_schema_type_lookup() {
        let mut schema = Schema::new("test".to_string(), 1, 1);
        schema.add_type(TypeDef::Primitive(PrimitiveDef::new(
            "uint64".to_string(),
            PrimitiveType::Uint64,
        )));

        assert!(schema.has_type("uint64"));
        assert!(!schema.has_type("unknown"));
        assert!(schema.get_type("uint64").is_some());
    }

    #[test]
    fn test_schema_build_type_map() {
        let mut schema = Schema::new("test".to_string(), 1, 1);
        schema.types.push(TypeDef::Primitive(PrimitiveDef::new(
            "int32".to_string(),
            PrimitiveType::Int32,
        )));
        schema.types.push(TypeDef::Primitive(PrimitiveDef::new(
            "int64".to_string(),
            PrimitiveType::Int64,
        )));

        schema.build_type_map();

        assert!(schema.has_type("int32"));
        assert!(schema.has_type("int64"));
    }

    #[test]
    fn test_byte_order_parse() {
        assert_eq!(
            ByteOrder::parse("littleEndian"),
            Some(ByteOrder::LittleEndian)
        );
        assert_eq!(ByteOrder::parse("bigEndian"), Some(ByteOrder::BigEndian));
        assert_eq!(ByteOrder::parse("le"), Some(ByteOrder::LittleEndian));
        assert_eq!(ByteOrder::parse("be"), Some(ByteOrder::BigEndian));
        assert_eq!(ByteOrder::parse("invalid"), None);
    }

    #[test]
    fn test_presence_parse() {
        assert_eq!(Presence::parse("required"), Some(Presence::Required));
        assert_eq!(Presence::parse("optional"), Some(Presence::Optional));
        assert_eq!(Presence::parse("constant"), Some(Presence::Constant));
        assert_eq!(Presence::parse("REQUIRED"), Some(Presence::Required));
        assert_eq!(Presence::parse("invalid"), None);
    }

    #[test]
    fn test_primitive_type_rust_type() {
        assert_eq!(PrimitiveType::Char.rust_type(), "u8");
        assert_eq!(PrimitiveType::Int8.rust_type(), "i8");
        assert_eq!(PrimitiveType::Uint64.rust_type(), "u64");
        assert_eq!(PrimitiveType::Float.rust_type(), "f32");
        assert_eq!(PrimitiveType::Double.rust_type(), "f64");
    }

    #[test]
    fn test_primitive_type_sbe_name() {
        assert_eq!(PrimitiveType::Char.sbe_name(), "char");
        assert_eq!(PrimitiveType::Int32.sbe_name(), "int32");
        assert_eq!(PrimitiveType::Uint64.sbe_name(), "uint64");
    }

    #[test]
    fn test_primitive_type_from_sbe_name() {
        assert_eq!(
            PrimitiveType::from_sbe_name("char"),
            Some(PrimitiveType::Char)
        );
        assert_eq!(
            PrimitiveType::from_sbe_name("int64"),
            Some(PrimitiveType::Int64)
        );
        assert_eq!(PrimitiveType::from_sbe_name("unknown"), None);
    }

    #[test]
    fn test_type_def_name() {
        let prim = TypeDef::Primitive(PrimitiveDef::new("test".to_string(), PrimitiveType::Int32));
        assert_eq!(prim.name(), "test");

        let comp = TypeDef::Composite(CompositeDef::new("decimal".to_string()));
        assert_eq!(comp.name(), "decimal");

        let enum_def = TypeDef::Enum(EnumDef::new("Side".to_string(), PrimitiveType::Uint8));
        assert_eq!(enum_def.name(), "Side");

        let set_def = TypeDef::Set(SetDef::new("Flags".to_string(), PrimitiveType::Uint8));
        assert_eq!(set_def.name(), "Flags");
    }

    #[test]
    fn test_enum_value_as_u64() {
        let val = EnumValue::new("Buy".to_string(), "1".to_string());
        assert_eq!(val.as_u64(), Some(1));

        let val2 = EnumValue::new("Invalid".to_string(), "abc".to_string());
        assert_eq!(val2.as_u64(), None);
    }

    #[test]
    fn test_set_def() {
        let mut set_def = SetDef::new("Flags".to_string(), PrimitiveType::Uint8);
        set_def.add_choice(SetChoice::new("Active".to_string(), 0));
        set_def.add_choice(SetChoice::new("Visible".to_string(), 1));

        assert_eq!(set_def.choices.len(), 2);
        assert_eq!(set_def.get_choice("Active").unwrap().bit_position, 0);
        assert_eq!(set_def.get_choice("Visible").unwrap().bit_position, 1);
    }

    #[test]
    fn test_composite_field() {
        let field = CompositeField::new("mantissa".to_string(), "int64".to_string(), 8);
        assert_eq!(field.name, "mantissa");
        assert_eq!(field.type_name, "int64");
        assert_eq!(field.encoded_length, 8);
    }
}
