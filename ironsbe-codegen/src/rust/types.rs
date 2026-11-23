//! Type code generation.

use ironsbe_schema::ir::{
    CompositeMember, MemberKind, SchemaIr, TypeKind, to_pascal_case, to_snake_case,
};
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
            if let TypeKind::Composite { members, .. } = &resolved_type.kind {
                // Skip messageHeader - it's provided by ironsbe_core::header::MessageHeader
                if resolved_type.name.eq_ignore_ascii_case("messageHeader") {
                    continue;
                }
                output.push_str(&self.generate_composite(
                    &resolved_type.name,
                    members,
                    resolved_type.encoded_length,
                ));
            }
        }

        output
    }

    /// Generates a composite type struct with zero-copy decoder and encoder.
    ///
    /// Each member gets a typed accessor at its resolved offset: a primitive
    /// reads and writes its value, an array its bytes, an enum or a set its
    /// type, a nested composite its own decoder and encoder, and a constant
    /// returns its value and takes no bytes.
    fn generate_composite(
        &self,
        name: &str,
        members: &[CompositeMember],
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

        for member in members {
            output.push_str(&self.member_getter(member));
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

        for member in members {
            output.push_str(&self.member_setter(member));
        }

        output.push_str("}\n\n");

        output
    }

    /// The getter of one composite member.
    fn member_getter(&self, member: &CompositeMember) -> String {
        // A zero-length member (the `varData` of a var data encoding) has no
        // fixed bytes to read.
        if member.encoded_length == 0 && !matches!(member.kind, MemberKind::Constant { .. }) {
            return String::new();
        }
        let field_name = to_snake_case(&member.name);
        let offset = member.offset;
        let (ret, value) = match &member.kind {
            MemberKind::Primitive {
                array_length: Some(_),
                ..
            } => (
                "&'a [u8]".to_owned(),
                format!(
                    "self.buffer.get(self.offset + {offset}..self.offset + {}).unwrap_or(&[])",
                    offset + member.encoded_length
                ),
            ),
            MemberKind::Primitive { primitive_type, .. } => (
                primitive_type.rust_type().to_owned(),
                format!(
                    "self.buffer.{}(self.offset + {offset})",
                    get_read_method(*primitive_type)
                ),
            ),
            MemberKind::Constant {
                primitive_type,
                value,
            } => match constant_value(*primitive_type, value) {
                Some(pair) => pair,
                None => return String::new(),
            },
            MemberKind::Ref { type_name } => {
                match self.ir.get_type(type_name).map(|t| (t, &t.kind)) {
                    Some((t, TypeKind::Enum { encoding, .. })) => (
                        t.rust_type.clone(),
                        format!(
                            "{}::from(self.buffer.{}(self.offset + {offset}))",
                            t.rust_type,
                            get_read_method(*encoding)
                        ),
                    ),
                    Some((t, TypeKind::Set { encoding, .. })) => (
                        t.rust_type.clone(),
                        format!(
                            "{}::from_raw(self.buffer.{}(self.offset + {offset}))",
                            t.rust_type,
                            get_read_method(*encoding)
                        ),
                    ),
                    Some((t, TypeKind::Composite { .. })) => (
                        format!("{}<'a>", t.rust_type),
                        format!("{}::wrap(self.buffer, self.offset + {offset})", t.rust_type),
                    ),
                    Some((t, TypeKind::Primitive(_))) if t.is_array => (
                        "&'a [u8]".to_owned(),
                        format!(
                            "self.buffer.get(self.offset + {offset}..self.offset + {}).unwrap_or(&[])",
                            offset + member.encoded_length
                        ),
                    ),
                    Some((_, TypeKind::Primitive(prim))) => (
                        prim.rust_type().to_owned(),
                        format!(
                            "self.buffer.{}(self.offset + {offset})",
                            get_read_method(*prim)
                        ),
                    ),
                    None => return String::new(),
                }
            }
        };
        let mut out = String::new();
        out.push_str(&format!("    /// Gets the {} member.\n", member.name));
        out.push_str("    #[inline(always)]\n");
        out.push_str("    #[must_use]\n");
        out.push_str(&format!("    pub fn {field_name}(&self) -> {ret} {{\n"));
        out.push_str(&format!("        {value}\n"));
        out.push_str("    }\n\n");
        out
    }

    /// The setter of one composite member; constants have none.
    fn member_setter(&self, member: &CompositeMember) -> String {
        if member.encoded_length == 0 {
            return String::new();
        }
        let field_name = to_snake_case(&member.name);
        let offset = member.offset;
        let bytes_setter = |len: usize| {
            let mut out = String::new();
            out.push_str(&format!(
                "    /// Sets the {} member's bytes.\n",
                member.name
            ));
            out.push_str("    #[inline(always)]\n");
            out.push_str(&format!(
                "    pub fn set_{field_name}(&mut self, value: &[u8]) -> &mut Self {{\n"
            ));
            // Copies what fits and zero-fills the rest; a buffer too short
            // for the member is left as it is.
            out.push_str(&format!(
                "        if let Some(dst) = self.buffer.get_mut(self.offset + {offset}..self.offset + {}) {{\n",
                offset + len
            ));
            out.push_str("            let copy_len = value.len().min(dst.len());\n");
            out.push_str("            let (head, tail) = dst.split_at_mut(copy_len);\n");
            out.push_str("            if let Some(src) = value.get(..copy_len) {\n");
            out.push_str("                head.copy_from_slice(src);\n");
            out.push_str("            }\n");
            out.push_str("            tail.fill(0);\n");
            out.push_str("        }\n");
            out.push_str("        self\n");
            out.push_str("    }\n\n");
            out
        };
        let scalar_setter = |rust_type: &str, write: String| {
            let mut out = String::new();
            out.push_str(&format!("    /// Sets the {} member.\n", member.name));
            out.push_str("    #[inline(always)]\n");
            out.push_str(&format!(
                "    pub fn set_{field_name}(&mut self, value: {rust_type}) -> &mut Self {{\n"
            ));
            out.push_str(&format!("        {write};\n"));
            out.push_str("        self\n");
            out.push_str("    }\n\n");
            out
        };
        match &member.kind {
            MemberKind::Constant { .. } => String::new(),
            MemberKind::Primitive {
                array_length: Some(_),
                ..
            } => bytes_setter(member.encoded_length),
            MemberKind::Primitive { primitive_type, .. } => scalar_setter(
                primitive_type.rust_type(),
                format!(
                    "self.buffer.{}(self.offset + {offset}, value)",
                    get_write_method(*primitive_type)
                ),
            ),
            MemberKind::Ref { type_name } => {
                match self.ir.get_type(type_name).map(|t| (t, &t.kind)) {
                    Some((t, TypeKind::Enum { encoding, .. })) => scalar_setter(
                        &t.rust_type,
                        format!(
                            "self.buffer.{}(self.offset + {offset}, {}::from(value))",
                            get_write_method(*encoding),
                            encoding.rust_type()
                        ),
                    ),
                    Some((t, TypeKind::Set { encoding, .. })) => scalar_setter(
                        &t.rust_type,
                        format!(
                            "self.buffer.{}(self.offset + {offset}, value.raw())",
                            get_write_method(*encoding)
                        ),
                    ),
                    Some((t, TypeKind::Composite { .. })) => {
                        let mut out = String::new();
                        out.push_str(&format!("    /// Encoder of the {} member.\n", member.name));
                        out.push_str("    #[inline(always)]\n");
                        out.push_str(&format!(
                            "    pub fn {field_name}(&mut self) -> {}Encoder<'_> {{\n",
                            t.rust_type
                        ));
                        out.push_str(&format!(
                            "        {}Encoder::wrap(self.buffer, self.offset + {offset})\n",
                            t.rust_type
                        ));
                        out.push_str("    }\n\n");
                        out
                    }
                    Some((t, TypeKind::Primitive(_))) if t.is_array => {
                        bytes_setter(member.encoded_length)
                    }
                    Some((_, TypeKind::Primitive(prim))) => scalar_setter(
                        prim.rust_type(),
                        format!(
                            "self.buffer.{}(self.offset + {offset}, value)",
                            get_write_method(*prim)
                        ),
                    ),
                    None => String::new(),
                }
            }
        }
    }
}

/// The return type and value of a constant member's getter: a number for a
/// numeric primitive, a `&'static str` for a char constant. `None` when the
/// constant cannot be expressed (no primitive type, or a value that does not
/// parse).
fn constant_value(prim: Option<PrimitiveType>, value: &str) -> Option<(String, String)> {
    let prim = prim?;
    match prim {
        PrimitiveType::Char => Some(("&'static str".to_owned(), format!("{value:?}"))),
        PrimitiveType::Float | PrimitiveType::Double => value.parse::<f64>().ok().map(|_| {
            (
                prim.rust_type().to_owned(),
                format!("{value}_{}", prim.rust_type()),
            )
        }),
        _ => value.parse::<i128>().ok().map(|_| {
            (
                prim.rust_type().to_owned(),
                format!("{value}_{}", prim.rust_type()),
            )
        }),
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
