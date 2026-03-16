//! Type code generation.

use ironsbe_schema::ir::{CompositeFieldInfo, SchemaIr, TypeKind, to_pascal_case, to_snake_case};
use ironsbe_schema::types::PrimitiveType;

/// Generator for type definitions.
pub struct TypeGenerator<'a> {
    ir: &'a SchemaIr,
}

impl<'a> TypeGenerator<'a> {
    /// Creates a new type generator.
    #[must_use]
    pub fn new(ir: &'a SchemaIr) -> Self {
        Self { ir }
    }

    /// Generates all type definitions.
    #[must_use]
    pub fn generate(&self) -> String {
        let mut output = String::new();

        for resolved_type in self.ir.types.values() {
            if let TypeKind::Composite { fields } = &resolved_type.kind {
                // Skip messageHeader - it's provided by ironsbe_core::header::MessageHeader
                if resolved_type.name.eq_ignore_ascii_case("messageHeader") {
                    continue;
                }
                output.push_str(&self.generate_composite(
                    &resolved_type.name,
                    fields,
                    resolved_type.encoded_length,
                ));
            }
        }

        output
    }

    /// Generates a composite type struct with zero-copy decoder and encoder.
    fn generate_composite(
        &self,
        name: &str,
        fields: &[CompositeFieldInfo],
        encoded_length: usize,
    ) -> String {
        let mut output = String::new();
        let struct_name = to_pascal_case(name);

        // Generate decoder struct
        output.push_str(&format!("/// {} Decoder (zero-copy).\n", struct_name));
        output.push_str("#[derive(Debug, Clone, Copy)]\n");
        output.push_str(&format!("pub struct {}<'a> {{\n", struct_name));
        output.push_str("    buffer: &'a [u8],\n");
        output.push_str("    offset: usize,\n");
        output.push_str("}\n\n");

        output.push_str(&format!("impl<'a> {}<'a> {{\n", struct_name));
        output.push_str(&format!(
            "    /// Encoded length of {} in bytes.\n",
            struct_name
        ));
        output.push_str(&format!(
            "    pub const ENCODED_LENGTH: usize = {};\n\n",
            encoded_length
        ));

        // Constructor
        output.push_str("    /// Wraps a buffer for zero-copy decoding.\n");
        output.push_str("    #[inline]\n");
        output.push_str("    #[must_use]\n");
        output.push_str("    pub fn wrap(buffer: &'a [u8], offset: usize) -> Self {\n");
        output.push_str("        Self { buffer, offset }\n");
        output.push_str("    }\n\n");

        // Field getters
        for field in fields {
            let field_name = to_snake_case(&field.name);
            let rust_type = field.primitive_type.rust_type();
            let read_method = get_read_method(field.primitive_type);

            output.push_str(&format!("    /// Gets the {} field.\n", field.name));
            output.push_str("    #[inline(always)]\n");
            output.push_str("    #[must_use]\n");
            output.push_str(&format!(
                "    pub fn {}(&self) -> {} {{\n",
                field_name, rust_type
            ));
            output.push_str(&format!(
                "        self.buffer.{}(self.offset + {})\n",
                read_method, field.offset
            ));
            output.push_str("    }\n\n");
        }

        output.push_str("}\n\n");

        // Generate encoder struct
        output.push_str(&format!("/// {} Encoder.\n", struct_name));
        output.push_str(&format!("pub struct {}Encoder<'a> {{\n", struct_name));
        output.push_str("    buffer: &'a mut [u8],\n");
        output.push_str("    offset: usize,\n");
        output.push_str("}\n\n");

        output.push_str(&format!("impl<'a> {}Encoder<'a> {{\n", struct_name));
        output.push_str(&format!(
            "    /// Encoded length of {} in bytes.\n",
            struct_name
        ));
        output.push_str(&format!(
            "    pub const ENCODED_LENGTH: usize = {};\n\n",
            encoded_length
        ));

        // Constructor
        output.push_str("    /// Wraps a buffer for encoding.\n");
        output.push_str("    #[inline]\n");
        output.push_str("    pub fn wrap(buffer: &'a mut [u8], offset: usize) -> Self {\n");
        output.push_str("        Self { buffer, offset }\n");
        output.push_str("    }\n\n");

        // Field setters
        for field in fields {
            let field_name = to_snake_case(&field.name);
            let rust_type = field.primitive_type.rust_type();
            let write_method = get_write_method(field.primitive_type);

            output.push_str(&format!("    /// Sets the {} field.\n", field.name));
            output.push_str("    #[inline(always)]\n");
            output.push_str(&format!(
                "    pub fn set_{}(&mut self, value: {}) -> &mut Self {{\n",
                field_name, rust_type
            ));
            output.push_str(&format!(
                "        self.buffer.{}(self.offset + {}, value);\n",
                write_method, field.offset
            ));
            output.push_str("        self\n");
            output.push_str("    }\n\n");
        }

        output.push_str("}\n\n");

        output
    }
}

/// Gets the read method name for a primitive type.
fn get_read_method(prim: PrimitiveType) -> &'static str {
    match prim {
        PrimitiveType::Char | PrimitiveType::Uint8 => "get_u8",
        PrimitiveType::Int8 => "get_i8",
        PrimitiveType::Uint16 => "get_u16_le",
        PrimitiveType::Int16 => "get_i16_le",
        PrimitiveType::Uint32 => "get_u32_le",
        PrimitiveType::Int32 => "get_i32_le",
        PrimitiveType::Uint64 => "get_u64_le",
        PrimitiveType::Int64 => "get_i64_le",
        PrimitiveType::Float => "get_f32_le",
        PrimitiveType::Double => "get_f64_le",
    }
}

/// Gets the write method name for a primitive type.
fn get_write_method(prim: PrimitiveType) -> &'static str {
    match prim {
        PrimitiveType::Char | PrimitiveType::Uint8 => "put_u8",
        PrimitiveType::Int8 => "put_i8",
        PrimitiveType::Uint16 => "put_u16_le",
        PrimitiveType::Int16 => "put_i16_le",
        PrimitiveType::Uint32 => "put_u32_le",
        PrimitiveType::Int32 => "put_i32_le",
        PrimitiveType::Uint64 => "put_u64_le",
        PrimitiveType::Int64 => "put_i64_le",
        PrimitiveType::Float => "put_f32_le",
        PrimitiveType::Double => "put_f64_le",
    }
}
