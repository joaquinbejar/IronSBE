//! Variable-length (`<data>`) field resolution and accessor emission.
//!
//! A `<data>` element references a composite such as `varStringEncoding`
//! whose `length` member decides how wide the length header is on the wire.
//! This module resolves that layout once per field and emits the getters,
//! setters and offset helpers used by message codecs.

use ironsbe_schema::ir::{ResolvedMessage, SchemaIr, TypeKind, to_snake_case};
use ironsbe_schema::types::PrimitiveType;

use crate::error::CodegenError;

/// Resolved wire layout of one `<data>` (variable-length) field.
pub(crate) struct VarDataInfo {
    /// Original schema name, used in doc comments.
    pub(crate) name: String,
    /// snake_case base name for the generated accessors.
    pub(crate) accessor: String,
    /// Field ID from the schema.
    pub(crate) id: u16,
    /// SBE name of the length primitive (`uint16`), used in doc comments.
    pub(crate) length_type: &'static str,
    /// Rust type of the length primitive (`u16`).
    pub(crate) length_rust_type: &'static str,
    /// Encoded width of the length header in bytes (1, 2 or 4).
    pub(crate) header_length: usize,
    /// `ReadBuffer` method that reads the length header.
    pub(crate) read_method: &'static str,
    /// `WriteBuffer` method that writes the length header.
    pub(crate) write_method: &'static str,
}

/// Resolves the length-header layout of every `<data>` field in `msg`.
///
/// # Errors
/// Returns [`CodegenError::UnknownType`] when the referenced type is not
/// declared, and [`CodegenError::Unsupported`] when it is not a composite,
/// has no length member, or its length member is not `uint8` / `uint16` /
/// `uint32`.
pub(crate) fn resolve_var_data(
    ir: &SchemaIr,
    msg: &ResolvedMessage,
) -> Result<Vec<VarDataInfo>, CodegenError> {
    msg.var_data
        .iter()
        .map(|vd| {
            let context = format!("message '{}', data '{}'", msg.name, vd.name);
            let field_path = format!("{}.{}", msg.name, vd.name);

            let resolved = ir
                .get_type(&vd.type_name)
                .ok_or_else(|| CodegenError::unknown_type(&vd.type_name, &field_path))?;

            let TypeKind::Composite { fields } = &resolved.kind else {
                return Err(CodegenError::unsupported(
                    format!("var data type '{}' is not a composite", vd.type_name),
                    context,
                ));
            };

            let length_field = fields
                .iter()
                .find(|f| f.name.eq_ignore_ascii_case("length"))
                .or_else(|| fields.first())
                .ok_or_else(|| {
                    CodegenError::unsupported(
                        format!("var data type '{}' has no length member", vd.type_name),
                        context.clone(),
                    )
                })?;

            let (header_length, read_method, write_method, length_rust_type, length_type) =
                match length_field.primitive_type {
                    PrimitiveType::Uint8 => (1, "get_u8", "put_u8", "u8", "uint8"),
                    PrimitiveType::Uint16 => (2, "get_u16_le", "put_u16_le", "u16", "uint16"),
                    PrimitiveType::Uint32 => (4, "get_u32_le", "put_u32_le", "u32", "uint32"),
                    other => {
                        return Err(CodegenError::unsupported(
                            format!("var data length encoding '{}'", other.sbe_name()),
                            context,
                        ));
                    }
                };

            Ok(VarDataInfo {
                name: vd.name.clone(),
                accessor: to_snake_case(&vd.name),
                id: vd.id,
                length_type,
                length_rust_type,
                header_length,
                read_method,
                write_method,
            })
        })
        .collect()
}

/// Generates the private `<name>_offset()` helper plus the public slice
/// and string accessors for the `index`-th var data field of a message.
pub(crate) fn generate_var_data_getter(
    index: usize,
    var_data: &[VarDataInfo],
    group_count: usize,
) -> String {
    let mut output = String::new();
    let Some(info) = var_data.get(index) else {
        return output;
    };

    // Offset helper: first var data field starts after the last group
    // (or after the fixed block); each later field starts after the
    // previous field's header + payload.
    output.push_str(&format!(
        "    /// Byte offset of the length header of var data field `{}`.\n",
        info.name
    ));
    output.push_str("    #[inline]\n");
    output.push_str(&format!(
        "    fn {}_offset(&self) -> usize {{\n",
        info.accessor
    ));
    match index.checked_sub(1).and_then(|i| var_data.get(i)) {
        Some(prev) => {
            output.push_str(&format!(
                "        let pos = self.{}_offset();\n",
                prev.accessor
            ));
            output.push_str(&format!(
                "        pos + {} + self.buffer.{}(pos) as usize\n",
                prev.header_length, prev.read_method
            ));
        }
        None if group_count > 0 => {
            output.push_str(&format!("        self.group_offset({})\n", group_count));
        }
        None => {
            output.push_str("        self.offset + Self::BLOCK_LENGTH as usize\n");
        }
    }
    output.push_str("    }\n\n");

    // Slice accessor
    output.push_str(&format!(
        "    /// Var data field: {} (id={}, length header: {}).\n",
        info.name, info.id, info.length_type
    ));
    output.push_str("    ///\n");
    output.push_str("    /// Returns the raw bytes. Var data fields follow all repeating groups\n");
    output.push_str("    /// in schema order.\n");
    output.push_str("    ///\n");
    output.push_str("    /// # Panics\n");
    output.push_str(
        "    /// Panics if the buffer is shorter than the encoded length header claims.\n",
    );
    output.push_str("    #[inline]\n");
    output.push_str("    #[must_use]\n");
    output.push_str(&format!(
        "    pub fn {}(&self) -> &'a [u8] {{\n",
        info.accessor
    ));
    output.push_str(&format!(
        "        let pos = self.{}_offset();\n",
        info.accessor
    ));
    output.push_str(&format!(
        "        let len = self.buffer.{}(pos) as usize;\n",
        info.read_method
    ));
    output.push_str(&format!(
        "        let start = pos + {};\n",
        info.header_length
    ));
    output.push_str("        &self.buffer[start..start + len]\n");
    output.push_str("    }\n\n");

    // String accessor
    output.push_str(&format!(
        "    /// Var data field `{}` as UTF-8 (empty string if not valid UTF-8).\n",
        info.name
    ));
    output.push_str("    #[inline]\n");
    output.push_str("    #[must_use]\n");
    output.push_str(&format!(
        "    pub fn {}_as_str(&self) -> &'a str {{\n",
        info.accessor
    ));
    output.push_str(&format!(
        "        std::str::from_utf8(self.{}()).unwrap_or(\"\")\n",
        info.accessor
    ));
    output.push_str("    }\n\n");

    output
}

/// Generates `SbeDecoder::encoded_length` for a message decoder.
///
/// Covers header + fixed block + every repeating group + every var data
/// field, reading the variable parts from the wire.
pub(crate) fn generate_decoder_encoded_length(
    var_data: &[VarDataInfo],
    group_count: usize,
) -> String {
    let mut output = String::new();

    output.push_str("    fn encoded_length(&self) -> usize {\n");
    match var_data.last() {
        Some(last) => {
            output.push_str(&format!(
                "        let pos = self.{}_offset();\n",
                last.accessor
            ));
            output.push_str(&format!(
                "        let end = pos + {} + self.buffer.{}(pos) as usize;\n",
                last.header_length, last.read_method
            ));
            output.push_str("        MessageHeader::ENCODED_LENGTH + (end - self.offset)\n");
        }
        None if group_count > 0 => {
            output.push_str(&format!(
                "        MessageHeader::ENCODED_LENGTH + (self.group_offset({}) - self.offset)\n",
                group_count
            ));
        }
        None => {
            output
                .push_str("        MessageHeader::ENCODED_LENGTH + Self::BLOCK_LENGTH as usize\n");
        }
    }
    output.push_str("    }\n");

    output
}

/// Generates `set_<name>` for one var data field on a message encoder.
pub(crate) fn generate_var_data_setter(info: &VarDataInfo) -> String {
    let mut output = String::new();

    output.push_str(&format!(
        "    /// Set var data field: {} (id={}, length header: {}).\n",
        info.name, info.id, info.length_type
    ));
    output.push_str("    ///\n");
    output.push_str("    /// Appends the length header followed by `value` at the write cursor.\n");
    output.push_str("    /// Var data fields must be written in schema order, after all\n");
    output.push_str("    /// repeating groups.\n");
    output.push_str("    ///\n");
    output.push_str("    /// # Panics\n");
    output.push_str("    /// Panics if `value.len()` does not fit in the length header, or if\n");
    output.push_str("    /// the buffer is too short.\n");
    output.push_str("    #[inline]\n");
    output.push_str(&format!(
        "    pub fn set_{}(&mut self, value: &[u8]) -> &mut Self {{\n",
        info.accessor
    ));
    output.push_str(&format!(
        "        let Ok(len) = {}::try_from(value.len()) else {{\n",
        info.length_rust_type
    ));
    output.push_str(&format!(
        "            panic!(\"var data field '{}': length {{}} exceeds {}::MAX\", value.len());\n",
        info.name, info.length_rust_type
    ));
    output.push_str("        };\n");
    output.push_str(&format!(
        "        self.buffer.{}(self.limit, len);\n",
        info.write_method
    ));
    output.push_str(&format!(
        "        let start = self.limit + {};\n",
        info.header_length
    ));
    output.push_str("        self.buffer.put_bytes(start, value);\n");
    output.push_str("        self.limit = start + value.len();\n");
    output.push_str("        self\n");
    output.push_str("    }\n\n");

    output
}
