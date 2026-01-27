//! Type code generation.

use ironsbe_schema::ir::{SchemaIr, TypeKind};

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
        let output = String::new();

        for resolved_type in self.ir.types.values() {
            if let TypeKind::Composite = resolved_type.kind {
                // Generate composite type struct
                // For now, composites are handled inline in field accessors
            }
        }

        output
    }
}
