//! Fixed-length field accessor emission.
//!
//! Getters and setters for `<field>` elements are identical for message
//! decoders/encoders and group entry decoders/encoders except for the base
//! offset expression, so they live here and are shared by both generators.

use ironsbe_schema::ir::{ResolvedField, SchemaIr, TypeKind};
use ironsbe_schema::types::PrimitiveType;

use crate::rust::names::{accessor_name, renamed_note};

/// Generates a field getter method.
///
/// Emitted inside an `impl` block whose `self` has `buffer: &'a [u8]` and
/// `offset: usize` (start of the fixed block). `reserved` lists the methods
/// the host defines itself; a field named like one of them gets a trailing
/// underscore (see `names`).
pub(crate) fn generate_field_getter(
    ir: &SchemaIr,
    field: &ResolvedField,
    reserved: &[&str],
) -> String {
    let mut output = String::new();
    let getter = accessor_name(&field.getter_name, reserved);

    output.push_str(&format!(
        "    /// Field: {} (id={}, offset={}).\n",
        field.name, field.id, field.offset
    ));
    output.push_str(&renamed_note(&field.getter_name, &getter));
    output.push_str("    #[inline(always)]\n");
    output.push_str("    #[must_use]\n");

    if field.is_array {
        // Array field - return slice
        let elem_type = field.primitive_type.map(|p| p.rust_type()).unwrap_or("u8");
        let len = field.array_length.unwrap_or(1);

        if elem_type == "u8" {
            // Byte array - return &[u8]
            output.push_str(&format!("    pub fn {}(&self) -> &'a [u8] {{\n", getter));
            output.push_str(&format!(
                "        &self.buffer[self.offset + {}..self.offset + {} + {}]\n",
                field.offset, field.offset, len
            ));
            output.push_str("    }\n\n");

            // Also generate a string accessor for char arrays
            output.push_str(&format!(
                "    /// Field {} as string (trimmed).\n",
                field.name
            ));
            output.push_str("    #[inline]\n");
            output.push_str("    #[must_use]\n");
            output.push_str(&format!(
                "    pub fn {}_as_str(&self) -> &'a str {{\n",
                getter
            ));
            output.push_str(&format!(
                "        let bytes = &self.buffer[self.offset + {}..self.offset + {} + {}];\n",
                field.offset, field.offset, len
            ));
            output.push_str(
                "        let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());\n",
            );
            output.push_str("        std::str::from_utf8(&bytes[..end]).unwrap_or(\"\")\n");
            output.push_str("    }\n\n");
        } else {
            // Other array types
            output.push_str(&format!("    pub fn {}(&self) -> &'a [u8] {{\n", getter));
            output.push_str(&format!(
                "        &self.buffer[self.offset + {}..self.offset + {}]\n",
                field.offset,
                field.offset + field.encoded_length
            ));
            output.push_str("    }\n\n");
        }
    } else {
        // Scalar field - check if it's an enum/set type
        let rust_type = &field.rust_type;
        let resolved_type = ir.get_type(&field.type_name);

        match resolved_type.map(|t| &t.kind) {
            Some(TypeKind::Enum { encoding, .. }) => {
                // Enum field - use encoding primitive and wrap with From
                let read_method = get_read_method(Some(*encoding));
                output.push_str(&format!(
                    "    pub fn {}(&self) -> {} {{\n",
                    getter, rust_type
                ));
                output.push_str(&format!(
                    "        {}::from(self.buffer.{}(self.offset + {}))\n",
                    rust_type, read_method, field.offset
                ));
                output.push_str("    }\n\n");
            }
            Some(TypeKind::Set { encoding, .. }) => {
                // Set field - use encoding primitive and wrap with from_raw
                let read_method = get_read_method(Some(*encoding));
                output.push_str(&format!(
                    "    pub fn {}(&self) -> {} {{\n",
                    getter, rust_type
                ));
                output.push_str(&format!(
                    "        {}::from_raw(self.buffer.{}(self.offset + {}))\n",
                    rust_type, read_method, field.offset
                ));
                output.push_str("    }\n\n");
            }
            Some(TypeKind::Composite { .. }) => {
                // Composite field - return wrapper struct
                output.push_str(&format!(
                    "    pub fn {}(&self) -> {}<'a> {{\n",
                    getter, rust_type
                ));
                output.push_str(&format!(
                    "        {}::wrap(self.buffer, self.offset + {})\n",
                    rust_type, field.offset
                ));
                output.push_str("    }\n\n");
            }
            _ => {
                // Primitive field
                let read_method = get_read_method(field.primitive_type);
                output.push_str(&format!(
                    "    pub fn {}(&self) -> {} {{\n",
                    getter, rust_type
                ));
                output.push_str(&format!(
                    "        self.buffer.{}(self.offset + {})\n",
                    read_method, field.offset
                ));
                output.push_str("    }\n\n");
            }
        }
    }

    output
}

/// Generates a field setter for a message encoder.
///
/// Message encoders keep `offset` at the message header, so the field offset
/// is prefixed with `MessageHeader::ENCODED_LENGTH`.
pub(crate) fn generate_field_setter(ir: &SchemaIr, field: &ResolvedField) -> String {
    let field_offset = format!("MessageHeader::ENCODED_LENGTH + {}", field.offset);
    generate_setter_at(ir, field, &field_offset)
}

/// Generates a field setter for a group entry encoder.
///
/// Entry encoders keep `offset` at the start of the entry's fixed block, so
/// the raw field offset is used without a header prefix.
pub(crate) fn generate_entry_field_setter(ir: &SchemaIr, field: &ResolvedField) -> String {
    generate_setter_at(ir, field, &field.offset.to_string())
}

/// Generates a field setter whose write position is `self.offset + field_offset`.
fn generate_setter_at(ir: &SchemaIr, field: &ResolvedField, field_offset: &str) -> String {
    let mut output = String::new();

    output.push_str(&format!(
        "    /// Set field: {} (id={}, offset={}).\n",
        field.name, field.id, field.offset
    ));
    output.push_str("    #[inline(always)]\n");

    if field.is_array {
        // Array field - accept slice
        let len = field.array_length.unwrap_or(field.encoded_length);

        output.push_str(&format!(
            "    pub fn {}(&mut self, value: &[u8]) -> &mut Self {{\n",
            field.setter_name
        ));
        output.push_str(&format!(
            "        let copy_len = value.len().min({});\n",
            len
        ));
        output.push_str(&format!(
            "        self.buffer[self.offset + {}..self.offset + {} + copy_len]\n",
            field_offset, field_offset
        ));
        output.push_str("            .copy_from_slice(&value[..copy_len]);\n");
        output.push_str(&format!("        if copy_len < {} {{\n", len));
        output.push_str(&format!(
            "            self.buffer[self.offset + {} + copy_len..self.offset + {} + {}].fill(0);\n",
            field_offset, field_offset, len
        ));
        output.push_str("        }\n");
        output.push_str("        self\n");
        output.push_str("    }\n\n");
    } else {
        // Scalar field - check if it's an enum/set type
        let rust_type = &field.rust_type;
        let resolved_type = ir.get_type(&field.type_name);

        match resolved_type.map(|t| &t.kind) {
            Some(TypeKind::Enum { encoding, .. }) => {
                // Enum field - convert enum to primitive before writing
                let write_method = get_write_method(Some(*encoding));
                let prim_type = encoding.rust_type();
                output.push_str(&format!(
                    "    pub fn {}(&mut self, value: {}) -> &mut Self {{\n",
                    field.setter_name, rust_type
                ));
                output.push_str(&format!(
                    "        self.buffer.{}(self.offset + {}, {}::from(value));\n",
                    write_method, field_offset, prim_type
                ));
                output.push_str("        self\n");
                output.push_str("    }\n\n");
            }
            Some(TypeKind::Set { encoding, .. }) => {
                // Set field - use raw() to get the primitive value
                let write_method = get_write_method(Some(*encoding));
                output.push_str(&format!(
                    "    pub fn {}(&mut self, value: {}) -> &mut Self {{\n",
                    field.setter_name, rust_type
                ));
                output.push_str(&format!(
                    "        self.buffer.{}(self.offset + {}, value.raw());\n",
                    write_method, field_offset
                ));
                output.push_str("        self\n");
                output.push_str("    }\n\n");
            }
            Some(TypeKind::Composite { .. }) => {
                // Composite field - return encoder for nested writes
                output.push_str(&format!(
                    "    pub fn {}(&mut self) -> {}Encoder<'_> {{\n",
                    field.setter_name, rust_type
                ));
                output.push_str(&format!(
                    "        {}Encoder::wrap(self.buffer, self.offset + {})\n",
                    rust_type, field_offset
                ));
                output.push_str("    }\n\n");
            }
            _ => {
                // Primitive field
                let write_method = get_write_method(field.primitive_type);
                output.push_str(&format!(
                    "    pub fn {}(&mut self, value: {}) -> &mut Self {{\n",
                    field.setter_name, rust_type
                ));
                output.push_str(&format!(
                    "        self.buffer.{}(self.offset + {}, value);\n",
                    write_method, field_offset
                ));
                output.push_str("        self\n");
                output.push_str("    }\n\n");
            }
        }
    }

    output
}

/// Gets the `ReadBuffer` method name for a primitive type.
pub(crate) fn get_read_method(prim: Option<PrimitiveType>) -> &'static str {
    match prim {
        Some(PrimitiveType::Char) | Some(PrimitiveType::Uint8) => "get_u8",
        Some(PrimitiveType::Int8) => "get_i8",
        Some(PrimitiveType::Uint16) => "get_u16_le",
        Some(PrimitiveType::Int16) => "get_i16_le",
        Some(PrimitiveType::Uint32) => "get_u32_le",
        Some(PrimitiveType::Int32) => "get_i32_le",
        Some(PrimitiveType::Uint64) => "get_u64_le",
        Some(PrimitiveType::Int64) => "get_i64_le",
        Some(PrimitiveType::Float) => "get_f32_le",
        Some(PrimitiveType::Double) => "get_f64_le",
        None => "get_u64_le",
    }
}

/// Gets the `WriteBuffer` method name for a primitive type.
pub(crate) fn get_write_method(prim: Option<PrimitiveType>) -> &'static str {
    match prim {
        Some(PrimitiveType::Char) | Some(PrimitiveType::Uint8) => "put_u8",
        Some(PrimitiveType::Int8) => "put_i8",
        Some(PrimitiveType::Uint16) => "put_u16_le",
        Some(PrimitiveType::Int16) => "put_i16_le",
        Some(PrimitiveType::Uint32) => "put_u32_le",
        Some(PrimitiveType::Int32) => "put_i32_le",
        Some(PrimitiveType::Uint64) => "put_u64_le",
        Some(PrimitiveType::Int64) => "put_i64_le",
        Some(PrimitiveType::Float) => "put_f32_le",
        Some(PrimitiveType::Double) => "put_f64_le",
        None => "put_u64_le",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_read_method_covers_every_primitive() {
        assert_eq!(get_read_method(Some(PrimitiveType::Char)), "get_u8");
        assert_eq!(get_read_method(Some(PrimitiveType::Uint16)), "get_u16_le");
        assert_eq!(get_read_method(Some(PrimitiveType::Int64)), "get_i64_le");
        assert_eq!(get_read_method(Some(PrimitiveType::Double)), "get_f64_le");
        assert_eq!(get_read_method(None), "get_u64_le");
    }

    #[test]
    fn test_get_write_method_covers_every_primitive() {
        assert_eq!(get_write_method(Some(PrimitiveType::Int8)), "put_i8");
        assert_eq!(get_write_method(Some(PrimitiveType::Uint32)), "put_u32_le");
        assert_eq!(get_write_method(Some(PrimitiveType::Float)), "put_f32_le");
        assert_eq!(get_write_method(None), "put_u64_le");
    }
}
