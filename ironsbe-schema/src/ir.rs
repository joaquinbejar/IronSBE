//! Intermediate representation for code generation.
//!
//! This module provides a flattened, resolved representation of the schema
//! that is easier to use for code generation.

use crate::types::{PrimitiveType, Schema, TypeDef};
use std::collections::HashMap;

/// Intermediate representation of a schema for code generation.
#[derive(Debug, Clone)]
pub struct SchemaIr {
    /// Package name.
    pub package: String,
    /// Schema ID.
    pub schema_id: u16,
    /// Schema version.
    pub schema_version: u16,
    /// Resolved types with their full information.
    pub types: HashMap<String, ResolvedType>,
    /// Messages with resolved field types.
    pub messages: Vec<ResolvedMessage>,
}

impl SchemaIr {
    /// Creates an intermediate representation from a parsed schema.
    #[must_use]
    pub fn from_schema(schema: &Schema) -> Self {
        let mut ir = Self {
            package: schema.package.clone(),
            schema_id: schema.id,
            schema_version: schema.version,
            types: HashMap::new(),
            messages: Vec::new(),
        };

        // Resolve types
        for type_def in &schema.types {
            let resolved = ResolvedType::from_type_def(type_def);
            ir.types.insert(resolved.name.clone(), resolved);
        }

        // Resolve messages
        for msg in &schema.messages {
            ir.messages
                .push(ResolvedMessage::from_message_def(msg, &ir.types));
        }

        ir
    }

    /// Gets a resolved type by name.
    #[must_use]
    pub fn get_type(&self, name: &str) -> Option<&ResolvedType> {
        self.types.get(name)
    }
}

/// Resolved type information.
#[derive(Debug, Clone)]
pub struct ResolvedType {
    /// Type name.
    pub name: String,
    /// Type kind.
    pub kind: TypeKind,
    /// Encoded length in bytes.
    pub encoded_length: usize,
    /// Rust type representation.
    pub rust_type: String,
    /// Whether this is an array type.
    pub is_array: bool,
    /// Array length (if array).
    pub array_length: Option<usize>,
}

impl ResolvedType {
    /// Creates a resolved type from a type definition.
    #[must_use]
    pub fn from_type_def(type_def: &TypeDef) -> Self {
        match type_def {
            TypeDef::Primitive(p) => Self {
                name: p.name.clone(),
                kind: TypeKind::Primitive(p.primitive_type),
                encoded_length: p.encoded_length(),
                rust_type: if p.is_array() {
                    format!(
                        "[{}; {}]",
                        p.primitive_type.rust_type(),
                        p.length.unwrap_or(1)
                    )
                } else {
                    p.primitive_type.rust_type().to_string()
                },
                is_array: p.is_array(),
                array_length: p.length,
            },
            TypeDef::Composite(c) => Self {
                name: c.name.clone(),
                kind: TypeKind::Composite,
                encoded_length: c.encoded_length(),
                rust_type: to_pascal_case(&c.name),
                is_array: false,
                array_length: None,
            },
            TypeDef::Enum(e) => Self {
                name: e.name.clone(),
                kind: TypeKind::Enum(e.encoding_type),
                encoded_length: e.encoding_type.size(),
                rust_type: to_pascal_case(&e.name),
                is_array: false,
                array_length: None,
            },
            TypeDef::Set(s) => Self {
                name: s.name.clone(),
                kind: TypeKind::Set(s.encoding_type),
                encoded_length: s.encoding_type.size(),
                rust_type: to_pascal_case(&s.name),
                is_array: false,
                array_length: None,
            },
        }
    }

    /// Creates a resolved type for a built-in primitive.
    #[must_use]
    pub fn from_primitive(prim: PrimitiveType) -> Self {
        Self {
            name: prim.sbe_name().to_string(),
            kind: TypeKind::Primitive(prim),
            encoded_length: prim.size(),
            rust_type: prim.rust_type().to_string(),
            is_array: false,
            array_length: None,
        }
    }
}

/// Type kind enumeration.
#[derive(Debug, Clone, Copy)]
pub enum TypeKind {
    /// Primitive type.
    Primitive(PrimitiveType),
    /// Composite type.
    Composite,
    /// Enum type with encoding.
    Enum(PrimitiveType),
    /// Set (bitfield) type with encoding.
    Set(PrimitiveType),
}

/// Resolved message information.
#[derive(Debug, Clone)]
pub struct ResolvedMessage {
    /// Message name.
    pub name: String,
    /// Template ID.
    pub template_id: u16,
    /// Block length.
    pub block_length: u16,
    /// Resolved fields.
    pub fields: Vec<ResolvedField>,
    /// Resolved groups.
    pub groups: Vec<ResolvedGroup>,
    /// Variable data fields.
    pub var_data: Vec<ResolvedVarData>,
}

impl ResolvedMessage {
    /// Creates a resolved message from a message definition.
    #[must_use]
    pub fn from_message_def(
        msg: &crate::messages::MessageDef,
        types: &HashMap<String, ResolvedType>,
    ) -> Self {
        let fields = msg
            .fields
            .iter()
            .map(|f| ResolvedField::from_field_def(f, types))
            .collect();

        let groups = msg
            .groups
            .iter()
            .map(|g| ResolvedGroup::from_group_def(g, types))
            .collect();

        let var_data = msg
            .data_fields
            .iter()
            .map(|d| ResolvedVarData {
                name: d.name.clone(),
                id: d.id,
                type_name: d.type_name.clone(),
            })
            .collect();

        Self {
            name: msg.name.clone(),
            template_id: msg.id,
            block_length: msg.block_length,
            fields,
            groups,
            var_data,
        }
    }

    /// Returns the decoder struct name.
    #[must_use]
    pub fn decoder_name(&self) -> String {
        format!("{}Decoder", self.name)
    }

    /// Returns the encoder struct name.
    #[must_use]
    pub fn encoder_name(&self) -> String {
        format!("{}Encoder", self.name)
    }
}

/// Resolved field information.
#[derive(Debug, Clone)]
pub struct ResolvedField {
    /// Field name.
    pub name: String,
    /// Field ID.
    pub id: u16,
    /// Type name.
    pub type_name: String,
    /// Offset in bytes.
    pub offset: usize,
    /// Encoded length in bytes.
    pub encoded_length: usize,
    /// Rust type.
    pub rust_type: String,
    /// Getter method name.
    pub getter_name: String,
    /// Setter method name.
    pub setter_name: String,
    /// Whether the field is optional.
    pub is_optional: bool,
    /// Whether the field is an array.
    pub is_array: bool,
    /// Array length (if array).
    pub array_length: Option<usize>,
    /// Primitive type (if applicable).
    pub primitive_type: Option<PrimitiveType>,
}

impl ResolvedField {
    /// Creates a resolved field from a field definition.
    #[must_use]
    pub fn from_field_def(
        field: &crate::messages::FieldDef,
        types: &HashMap<String, ResolvedType>,
    ) -> Self {
        let resolved_type = types.get(&field.type_name).cloned().or_else(|| {
            PrimitiveType::from_sbe_name(&field.type_name).map(ResolvedType::from_primitive)
        });

        let (encoded_length, rust_type, is_array, array_length, primitive_type) =
            if let Some(rt) = &resolved_type {
                (
                    rt.encoded_length,
                    rt.rust_type.clone(),
                    rt.is_array,
                    rt.array_length,
                    match rt.kind {
                        TypeKind::Primitive(p) => Some(p),
                        _ => None,
                    },
                )
            } else {
                (field.encoded_length, "u64".to_string(), false, None, None)
            };

        Self {
            name: field.name.clone(),
            id: field.id,
            type_name: field.type_name.clone(),
            offset: field.offset,
            encoded_length,
            rust_type,
            getter_name: to_snake_case(&field.name),
            setter_name: format!("set_{}", to_snake_case(&field.name)),
            is_optional: field.is_optional(),
            is_array,
            array_length,
            primitive_type,
        }
    }
}

/// Resolved group information.
#[derive(Debug, Clone)]
pub struct ResolvedGroup {
    /// Group name.
    pub name: String,
    /// Group ID.
    pub id: u16,
    /// Block length per entry.
    pub block_length: u16,
    /// Resolved fields.
    pub fields: Vec<ResolvedField>,
    /// Nested groups.
    pub nested_groups: Vec<ResolvedGroup>,
    /// Variable data fields.
    pub var_data: Vec<ResolvedVarData>,
}

impl ResolvedGroup {
    /// Creates a resolved group from a group definition.
    #[must_use]
    pub fn from_group_def(
        group: &crate::messages::GroupDef,
        types: &HashMap<String, ResolvedType>,
    ) -> Self {
        let fields = group
            .fields
            .iter()
            .map(|f| ResolvedField::from_field_def(f, types))
            .collect();

        let nested_groups = group
            .nested_groups
            .iter()
            .map(|g| ResolvedGroup::from_group_def(g, types))
            .collect();

        let var_data = group
            .data_fields
            .iter()
            .map(|d| ResolvedVarData {
                name: d.name.clone(),
                id: d.id,
                type_name: d.type_name.clone(),
            })
            .collect();

        Self {
            name: group.name.clone(),
            id: group.id,
            block_length: group.block_length,
            fields,
            nested_groups,
            var_data,
        }
    }

    /// Returns the decoder struct name.
    #[must_use]
    pub fn decoder_name(&self) -> String {
        format!("{}GroupDecoder", to_pascal_case(&self.name))
    }

    /// Returns the entry decoder struct name.
    #[must_use]
    pub fn entry_decoder_name(&self) -> String {
        format!("{}EntryDecoder", to_pascal_case(&self.name))
    }
}

/// Resolved variable data field.
#[derive(Debug, Clone)]
pub struct ResolvedVarData {
    /// Field name.
    pub name: String,
    /// Field ID.
    pub id: u16,
    /// Type name.
    pub type_name: String,
}

/// Converts a string to snake_case.
#[must_use]
pub fn to_snake_case(s: &str) -> String {
    let mut result = String::with_capacity(s.len() + 4);
    for (i, c) in s.chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            result.push('_');
        }
        result.push(c.to_ascii_lowercase());
    }
    result
}

/// Converts a string to PascalCase.
#[must_use]
pub fn to_pascal_case(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut capitalize_next = true;

    for c in s.chars() {
        if c == '_' || c == '-' {
            capitalize_next = true;
        } else if capitalize_next {
            result.push(c.to_ascii_uppercase());
            capitalize_next = false;
        } else {
            result.push(c);
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_schema;

    #[test]
    fn test_to_snake_case() {
        assert_eq!(to_snake_case("clOrdId"), "cl_ord_id");
        assert_eq!(to_snake_case("symbol"), "symbol");
        assert_eq!(to_snake_case("MDEntryPx"), "m_d_entry_px");
    }

    #[test]
    fn test_to_pascal_case() {
        assert_eq!(to_pascal_case("message_header"), "MessageHeader");
        assert_eq!(to_pascal_case("side"), "Side");
        assert_eq!(to_pascal_case("order-type"), "OrderType");
    }

    #[test]
    fn test_schema_ir_from_schema() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<sbe:messageSchema xmlns:sbe="http://fixprotocol.io/2016/sbe"
                   package="test" id="1" version="1" byteOrder="littleEndian">
    <types>
        <type name="uint64" primitiveType="uint64"/>
    </types>
    <sbe:message name="Test" id="1" blockLength="8">
        <field name="value" id="1" type="uint64" offset="0"/>
    </sbe:message>
</sbe:messageSchema>"#;

        let schema = parse_schema(xml).expect("Failed to parse");
        let ir = SchemaIr::from_schema(&schema);

        assert_eq!(ir.package, "test");
        assert_eq!(ir.schema_id, 1);
        assert_eq!(ir.schema_version, 1);
        assert!(!ir.messages.is_empty());
    }

    #[test]
    fn test_resolved_type_from_primitive() {
        let resolved = ResolvedType::from_primitive(PrimitiveType::Uint64);
        assert_eq!(resolved.name, "uint64");
        assert_eq!(resolved.encoded_length, 8);
        assert_eq!(resolved.rust_type, "u64");
        assert!(!resolved.is_array);
    }

    #[test]
    fn test_type_kind_debug() {
        let kind = TypeKind::Primitive(PrimitiveType::Int32);
        let debug_str = format!("{:?}", kind);
        assert!(debug_str.contains("Primitive"));

        let kind = TypeKind::Composite;
        let debug_str = format!("{:?}", kind);
        assert!(debug_str.contains("Composite"));

        let kind = TypeKind::Enum(PrimitiveType::Uint8);
        let debug_str = format!("{:?}", kind);
        assert!(debug_str.contains("Enum"));

        let kind = TypeKind::Set(PrimitiveType::Uint16);
        let debug_str = format!("{:?}", kind);
        assert!(debug_str.contains("Set"));
    }

    #[test]
    fn test_resolved_type_clone() {
        let resolved = ResolvedType::from_primitive(PrimitiveType::Float);
        let cloned = resolved.clone();
        assert_eq!(resolved.name, cloned.name);
        assert_eq!(resolved.encoded_length, cloned.encoded_length);
    }

    #[test]
    fn test_schema_ir_with_enum() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<sbe:messageSchema xmlns:sbe="http://fixprotocol.io/2016/sbe"
                   package="test" id="1" version="1" byteOrder="littleEndian">
    <types>
        <enum name="Side" encodingType="uint8">
            <validValue name="Buy">1</validValue>
            <validValue name="Sell">2</validValue>
        </enum>
    </types>
    <sbe:message name="Test" id="1" blockLength="1">
        <field name="side" id="1" type="Side" offset="0"/>
    </sbe:message>
</sbe:messageSchema>"#;

        let schema = parse_schema(xml).expect("Failed to parse");
        let ir = SchemaIr::from_schema(&schema);

        assert!(ir.types.contains_key("Side"));
    }

    #[test]
    fn test_schema_ir_with_composite() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<sbe:messageSchema xmlns:sbe="http://fixprotocol.io/2016/sbe"
                   package="test" id="1" version="1" byteOrder="littleEndian">
    <types>
        <composite name="Decimal">
            <type name="mantissa" primitiveType="int64"/>
            <type name="exponent" primitiveType="int8"/>
        </composite>
    </types>
    <sbe:message name="Test" id="1" blockLength="9">
        <field name="price" id="1" type="Decimal" offset="0"/>
    </sbe:message>
</sbe:messageSchema>"#;

        let schema = parse_schema(xml).expect("Failed to parse");
        let ir = SchemaIr::from_schema(&schema);

        assert!(ir.types.contains_key("Decimal"));
    }
}
