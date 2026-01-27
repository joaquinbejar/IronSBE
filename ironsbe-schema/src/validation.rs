//! Schema validation utilities.
//!
//! This module provides validation functions for SBE schemas to ensure
//! correctness and consistency.

use crate::error::SchemaError;
use crate::types::Schema;

/// Validates a parsed schema for correctness.
///
/// # Arguments
/// * `schema` - The schema to validate
///
/// # Returns
/// Ok(()) if valid, or SchemaError describing the issue.
///
/// # Errors
/// Returns `SchemaError` if validation fails.
pub fn validate_schema(schema: &Schema) -> Result<(), SchemaError> {
    validate_types(schema)?;
    validate_messages(schema)?;
    Ok(())
}

/// Validates all type definitions in the schema.
fn validate_types(schema: &Schema) -> Result<(), SchemaError> {
    for type_def in &schema.types {
        match type_def {
            crate::types::TypeDef::Composite(composite) => {
                validate_composite(schema, composite)?;
            }
            crate::types::TypeDef::Enum(enum_def) => {
                validate_enum(enum_def)?;
            }
            crate::types::TypeDef::Set(set_def) => {
                validate_set(set_def)?;
            }
            _ => {}
        }
    }
    Ok(())
}

/// Validates a composite type definition.
fn validate_composite(
    _schema: &Schema,
    composite: &crate::types::CompositeDef,
) -> Result<(), SchemaError> {
    let mut expected_offset = 0;

    for field in &composite.fields {
        if let Some(offset) = field.offset {
            if offset < expected_offset {
                return Err(SchemaError::InvalidOffset {
                    field: field.name.clone(),
                    offset,
                });
            }
            expected_offset = offset + field.encoded_length;
        } else {
            expected_offset += field.encoded_length;
        }
    }

    Ok(())
}

/// Validates an enum type definition.
fn validate_enum(enum_def: &crate::types::EnumDef) -> Result<(), SchemaError> {
    use std::collections::HashSet;

    let mut seen_names = HashSet::new();
    let mut seen_values = HashSet::new();

    for value in &enum_def.valid_values {
        if !seen_names.insert(&value.name) {
            return Err(SchemaError::Validation {
                message: format!(
                    "Duplicate enum value name '{}' in enum '{}'",
                    value.name, enum_def.name
                ),
            });
        }

        if !seen_values.insert(&value.value) {
            return Err(SchemaError::Validation {
                message: format!(
                    "Duplicate enum value '{}' in enum '{}'",
                    value.value, enum_def.name
                ),
            });
        }
    }

    Ok(())
}

/// Validates a set type definition.
fn validate_set(set_def: &crate::types::SetDef) -> Result<(), SchemaError> {
    use std::collections::HashSet;

    let max_bits = set_def.encoding_type.size() * 8;
    let mut seen_positions = HashSet::new();

    for choice in &set_def.choices {
        if choice.bit_position as usize >= max_bits {
            return Err(SchemaError::Validation {
                message: format!(
                    "Bit position {} exceeds maximum {} for set '{}'",
                    choice.bit_position,
                    max_bits - 1,
                    set_def.name
                ),
            });
        }

        if !seen_positions.insert(choice.bit_position) {
            return Err(SchemaError::Validation {
                message: format!(
                    "Duplicate bit position {} in set '{}'",
                    choice.bit_position, set_def.name
                ),
            });
        }
    }

    Ok(())
}

/// Validates all message definitions in the schema.
fn validate_messages(schema: &Schema) -> Result<(), SchemaError> {
    use std::collections::HashSet;

    let mut seen_ids = HashSet::new();
    let mut seen_names = HashSet::new();

    for msg in &schema.messages {
        if !seen_ids.insert(msg.id) {
            return Err(SchemaError::Validation {
                message: format!("Duplicate message ID {} for message '{}'", msg.id, msg.name),
            });
        }

        if !seen_names.insert(&msg.name) {
            return Err(SchemaError::Validation {
                message: format!("Duplicate message name '{}'", msg.name),
            });
        }

        validate_message_fields(schema, msg)?;
    }

    Ok(())
}

/// Validates fields within a message.
fn validate_message_fields(
    schema: &Schema,
    msg: &crate::messages::MessageDef,
) -> Result<(), SchemaError> {
    let mut max_offset = 0;

    for field in &msg.fields {
        // Check type exists
        if !schema.has_type(&field.type_name) {
            // Check if it's a built-in primitive
            if crate::types::PrimitiveType::from_sbe_name(&field.type_name).is_none() {
                return Err(SchemaError::TypeNotFound {
                    name: field.type_name.clone(),
                });
            }
        }

        // Check offset ordering
        if field.offset < max_offset && field.encoded_length > 0 {
            return Err(SchemaError::InvalidOffset {
                field: field.name.clone(),
                offset: field.offset,
            });
        }

        max_offset = field.offset + field.encoded_length;
    }

    // Check block length
    if max_offset > msg.block_length as usize {
        return Err(SchemaError::BlockLengthMismatch {
            message: msg.name.clone(),
            declared: msg.block_length,
            calculated: max_offset as u16,
        });
    }

    // Validate groups
    for group in &msg.groups {
        validate_group_fields(schema, group)?;
    }

    Ok(())
}

/// Validates fields within a group.
fn validate_group_fields(
    schema: &Schema,
    group: &crate::messages::GroupDef,
) -> Result<(), SchemaError> {
    for field in &group.fields {
        if !schema.has_type(&field.type_name)
            && crate::types::PrimitiveType::from_sbe_name(&field.type_name).is_none()
        {
            return Err(SchemaError::TypeNotFound {
                name: field.type_name.clone(),
            });
        }
    }

    // Validate nested groups
    for nested in &group.nested_groups {
        validate_group_fields(schema, nested)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_schema;

    #[test]
    fn test_validate_valid_schema() {
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
        assert!(validate_schema(&schema).is_ok());
    }

    #[test]
    fn test_validate_duplicate_message_id() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<sbe:messageSchema xmlns:sbe="http://fixprotocol.io/2016/sbe"
                   package="test" id="1" version="1" byteOrder="littleEndian">
    <types>
        <type name="uint64" primitiveType="uint64"/>
    </types>
    <sbe:message name="Test1" id="1" blockLength="8">
        <field name="value" id="1" type="uint64" offset="0"/>
    </sbe:message>
    <sbe:message name="Test2" id="1" blockLength="8">
        <field name="value" id="1" type="uint64" offset="0"/>
    </sbe:message>
</sbe:messageSchema>"#;

        let schema = parse_schema(xml).expect("Failed to parse");
        let result = validate_schema(&schema);
        assert!(result.is_err());
    }
}
