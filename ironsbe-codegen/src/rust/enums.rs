//! Enum and set code generation.

use ironsbe_schema::ir::{SchemaIr, TypeKind, to_pascal_case};
use ironsbe_schema::types::PrimitiveType;

/// Generator for enum and set definitions.
pub struct EnumGenerator<'a> {
    ir: &'a SchemaIr,
}

impl<'a> EnumGenerator<'a> {
    /// Creates a new enum generator.
    #[must_use]
    pub fn new(ir: &'a SchemaIr) -> Self {
        Self { ir }
    }

    /// Generates all enum and set definitions.
    #[must_use]
    pub fn generate(&self) -> String {
        let mut output = String::new();

        for resolved_type in self.ir.types.values() {
            match resolved_type.kind {
                TypeKind::Enum(encoding) => {
                    output.push_str(&self.generate_enum(&resolved_type.name, encoding));
                }
                TypeKind::Set(encoding) => {
                    output.push_str(&self.generate_set(&resolved_type.name, encoding));
                }
                _ => {}
            }
        }

        output
    }

    /// Generates an enum definition.
    fn generate_enum(&self, name: &str, encoding: PrimitiveType) -> String {
        let mut output = String::new();
        let rust_name = to_pascal_case(name);
        let rust_type = encoding.rust_type();

        output.push_str(&format!("/// {} enum.\n", rust_name));
        output.push_str("#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]\n");
        output.push_str(&format!("#[repr({})]\n", rust_type));
        output.push_str(&format!("pub enum {} {{\n", rust_name));

        // We need to get the actual enum values from the schema
        // For now, generate a placeholder
        output.push_str("    // Values generated from schema\n");
        output.push_str("}\n\n");

        // Implement From trait
        output.push_str(&format!("impl From<{}> for {} {{\n", rust_type, rust_name));
        output.push_str(&format!("    fn from(value: {}) -> Self {{\n", rust_type));
        output.push_str("        // Match implementation\n");
        output.push_str("        unsafe { std::mem::transmute(value) }\n");
        output.push_str("    }\n");
        output.push_str("}\n\n");

        output.push_str(&format!("impl From<{}> for {} {{\n", rust_name, rust_type));
        output.push_str(&format!("    fn from(value: {}) -> Self {{\n", rust_name));
        output.push_str("        value as Self\n");
        output.push_str("    }\n");
        output.push_str("}\n\n");

        output
    }

    /// Generates a set (bitfield) definition.
    fn generate_set(&self, name: &str, encoding: PrimitiveType) -> String {
        let mut output = String::new();
        let rust_name = to_pascal_case(name);
        let rust_type = encoding.rust_type();

        output.push_str(&format!("/// {} bitfield set.\n", rust_name));
        output.push_str("#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]\n");
        output.push_str(&format!("pub struct {}({});\n\n", rust_name, rust_type));

        output.push_str(&format!("impl {} {{\n", rust_name));
        output.push_str(&format!("    /// Creates a new empty {}.\n", rust_name));
        output.push_str("    #[must_use]\n");
        output.push_str("    pub const fn new() -> Self {\n");
        output.push_str("        Self(0)\n");
        output.push_str("    }\n\n");

        output.push_str(&format!("    /// Creates from raw {} value.\n", rust_type));
        output.push_str("    #[must_use]\n");
        output.push_str(&format!(
            "    pub const fn from_raw(value: {}) -> Self {{\n",
            rust_type
        ));
        output.push_str("        Self(value)\n");
        output.push_str("    }\n\n");

        output.push_str("    /// Returns the raw value.\n");
        output.push_str("    #[must_use]\n");
        output.push_str(&format!(
            "    pub const fn raw(&self) -> {} {{\n",
            rust_type
        ));
        output.push_str("        self.0\n");
        output.push_str("    }\n\n");

        output.push_str("    /// Checks if a bit is set.\n");
        output.push_str("    #[must_use]\n");
        output.push_str("    pub const fn is_set(&self, bit: u8) -> bool {\n");
        output.push_str("        (self.0 >> bit) & 1 != 0\n");
        output.push_str("    }\n\n");

        output.push_str("    /// Sets a bit.\n");
        output.push_str("    pub fn set(&mut self, bit: u8) {\n");
        output.push_str("        self.0 |= 1 << bit;\n");
        output.push_str("    }\n\n");

        output.push_str("    /// Clears a bit.\n");
        output.push_str("    pub fn clear(&mut self, bit: u8) {\n");
        output.push_str("        self.0 &= !(1 << bit);\n");
        output.push_str("    }\n");

        output.push_str("}\n\n");

        output
    }
}
