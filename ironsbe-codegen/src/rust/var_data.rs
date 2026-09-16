//! Variable-length (`<data>`) field resolution and accessor emission.
//!
//! A `<data>` element references a composite such as `varStringEncoding`
//! whose `length` member decides how wide the length header is on the wire.
//! This module resolves that layout once per field and emits the getters,
//! setters and offset helpers shared by message codecs and group entry
//! codecs. The emitters are parameterised by two expressions because the two
//! hosts differ only there:
//!
//! - the *block end* expression, where the variable section starts
//!   (`self.offset + Self::BLOCK_LENGTH as usize` on a message decoder,
//!   `self.offset + self.block_length as usize` on an entry decoder);
//! - the *cursor* expression on encoders (`self.limit` on a message encoder,
//!   `*self.limit` on group and entry encoders that borrow the parent's cursor).

use ironsbe_schema::ir::{ResolvedVarData, SchemaIr, TypeKind, to_snake_case};
use ironsbe_schema::types::PrimitiveType;

use crate::error::CodegenError;
use crate::rust::names::{accessor_name, renamed_note};

/// Resolved wire layout of one `<data>` (variable-length) field.
pub(crate) struct VarDataInfo {
    /// Original schema name, used in doc comments.
    pub(crate) name: String,
    /// snake_case base name for the setter and the private offset helper.
    pub(crate) accessor: String,
    /// Name of the public getter: `accessor`, or `accessor` with a trailing
    /// underscore when it collides with a method the host defines.
    pub(crate) getter: String,
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

/// Resolves the length-header layout of every `<data>` field of one owner
/// (a message or a group entry).
///
/// # Arguments
/// * `context` - Human-readable owner, e.g. `message 'Quote', group 'legs'`
/// * `path` - Dotted owner path for `UnknownType`, e.g. `Quote.legs`
/// * `var_data` - The owner's `<data>` fields in schema order
/// * `reserved` - Methods the owner's decoders define themselves, which a
///   getter must not shadow (see `names`)
///
/// # Errors
/// Returns [`CodegenError::UnknownType`] when the referenced type is not
/// declared, and [`CodegenError::Unsupported`] when it is not a composite,
/// has no length member, or its length member is not `uint8` / `uint16` /
/// `uint32`.
pub(crate) fn resolve_var_data(
    ir: &SchemaIr,
    context: &str,
    path: &str,
    var_data: &[ResolvedVarData],
    reserved: &[&str],
) -> Result<Vec<VarDataInfo>, CodegenError> {
    var_data
        .iter()
        .map(|vd| {
            let context = format!("{context}, data '{}'", vd.name);
            let field_path = format!("{path}.{}", vd.name);

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

            let accessor = to_snake_case(&vd.name);
            Ok(VarDataInfo {
                name: vd.name.clone(),
                getter: accessor_name(&accessor, reserved),
                accessor,
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

/// Generates the private `sbe_<name>_offset()` helper plus the public slice
/// and string accessors for the `index`-th var data field of an owner.
///
/// # Arguments
/// * `index` - Position of the field in `var_data`
/// * `var_data` - All var data fields of the owner, in schema order
/// * `group_count` - Number of repeating groups preceding the var data section
/// * `block_end_expr` - Expression for the byte offset just past the fixed block
pub(crate) fn generate_var_data_getter(
    index: usize,
    var_data: &[VarDataInfo],
    group_count: usize,
    block_end_expr: &str,
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
        "    fn sbe_{}_offset(&self) -> usize {{\n",
        info.accessor
    ));
    match index.checked_sub(1).and_then(|i| var_data.get(i)) {
        Some(prev) => {
            output.push_str(&format!(
                "        let pos = self.sbe_{}_offset();\n",
                prev.accessor
            ));
            output.push_str(&format!(
                "        pos + {} + self.buffer.{}(pos) as usize\n",
                prev.header_length, prev.read_method
            ));
        }
        None if group_count > 0 => {
            output.push_str(&format!("        self.sbe_group_offset({})\n", group_count));
        }
        None => {
            output.push_str(&format!("        {block_end_expr}\n"));
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
    output.push_str(
        "    /// Positioning walks any preceding variable-stride group on each call, so\n",
    );
    output.push_str("    /// on hot paths read the field once and cache the slice.\n");
    output.push_str(&renamed_note(&info.accessor, &info.getter));
    output.push_str("    ///\n");
    output.push_str("    /// # Panics\n");
    output.push_str(
        "    /// Panics if the buffer is shorter than the encoded length header claims.\n",
    );
    output.push_str("    #[inline]\n");
    output.push_str("    #[must_use]\n");
    output.push_str(&format!(
        "    pub fn {}(&self) -> &'a [u8] {{\n",
        info.getter
    ));
    output.push_str(&format!(
        "        let pos = self.sbe_{}_offset();\n",
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
        info.getter
    ));
    output.push_str("    }\n\n");

    output
}

/// Returns the code that computes the byte offset just past the variable
/// section of an owner: after the last var data field, else after the last
/// repeating group, else at the end of the fixed block.
///
/// The first element is a prelude of statements (possibly empty, each line
/// indented for a method body); the second is the final expression.
pub(crate) fn end_offset_parts(
    var_data: &[VarDataInfo],
    group_count: usize,
    block_end_expr: &str,
) -> (String, String) {
    match var_data.last() {
        Some(last) => (
            format!("        let pos = self.sbe_{}_offset();\n", last.accessor),
            format!(
                "pos + {} + self.buffer.{}(pos) as usize",
                last.header_length, last.read_method
            ),
        ),
        None if group_count > 0 => (
            String::new(),
            format!("self.sbe_group_offset({group_count})"),
        ),
        None => (String::new(), block_end_expr.to_string()),
    }
}

/// Generates `set_<name>` for one var data field on an encoder.
///
/// The host carries the variable-part guard (see `var_parts`): the setter
/// first fills every part skipped since the last write with an empty header,
/// then appends its own header and payload.
///
/// # Arguments
/// * `info` - Resolved field layout
/// * `cursor` - Place expression of the write cursor, e.g. `self.limit` on a
///   message encoder or `*self.limit` on an entry encoder
/// * `part_index` - Position of the field among the owner's variable parts
///   (repeating groups first, then var data fields)
pub(crate) fn generate_var_data_setter(
    info: &VarDataInfo,
    cursor: &str,
    part_index: usize,
) -> String {
    let mut output = String::new();

    output.push_str(&format!(
        "    /// Set var data field: {} (id={}, length header: {}).\n",
        info.name, info.id, info.length_type
    ));
    output.push_str("    ///\n");
    output.push_str("    /// Appends the length header followed by `value` at the write cursor.\n");
    output.push_str("    /// Var data fields must be written in schema order, after all\n");
    output.push_str("    /// repeating groups. Any group or var data field skipped before this\n");
    output.push_str("    /// one is written as empty first; a field never written encodes as\n");
    output.push_str("    /// empty when the entry is dropped or the message is finished.\n");
    output.push_str("    ///\n");
    output.push_str("    /// # Panics\n");
    output.push_str("    /// Panics if `value.len()` does not fit in the length header, if the\n");
    output.push_str("    /// buffer is too short, or if this field or a later part was already\n");
    output.push_str("    /// written (out-of-schema-order write).\n");
    output.push_str("    #[inline]\n");
    output.push_str(&format!(
        "    pub fn set_{}(&mut self, value: &[u8]) -> &mut Self {{\n",
        info.accessor
    ));
    output.push_str(&format!(
        "        self.sbe_advance_to({part_index}, \"var data field '{}'\");\n",
        info.name
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
        "        self.buffer.{}({cursor}, len);\n",
        info.write_method
    ));
    output.push_str(&format!(
        "        let start = {cursor} + {};\n",
        info.header_length
    ));
    output.push_str("        self.buffer.put_bytes(start, value);\n");
    output.push_str(&format!("        {cursor} = start + value.len();\n"));
    output.push_str(&format!("        self.written = {};\n", part_index + 1));
    output.push_str("        self\n");
    output.push_str("    }\n\n");

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(name: &str, header_length: usize) -> VarDataInfo {
        let (length_type, length_rust_type, read_method, write_method) = match header_length {
            1 => ("uint8", "u8", "get_u8", "put_u8"),
            4 => ("uint32", "u32", "get_u32_le", "put_u32_le"),
            _ => ("uint16", "u16", "get_u16_le", "put_u16_le"),
        };
        VarDataInfo {
            name: name.to_string(),
            getter: to_snake_case(name),
            accessor: to_snake_case(name),
            id: 1,
            length_type,
            length_rust_type,
            header_length,
            read_method,
            write_method,
        }
    }

    #[test]
    fn test_end_offset_parts_prefers_last_var_data() {
        let fields = [info("label", 2), info("payload", 1)];
        let (prelude, expr) = end_offset_parts(&fields, 3, "BLOCK_END");
        assert_eq!(prelude, "        let pos = self.sbe_payload_offset();\n");
        assert_eq!(expr, "pos + 1 + self.buffer.get_u8(pos) as usize");
    }

    #[test]
    fn test_end_offset_parts_falls_back_to_last_group() {
        let (prelude, expr) = end_offset_parts(&[], 2, "BLOCK_END");
        assert!(prelude.is_empty());
        assert_eq!(expr, "self.sbe_group_offset(2)");
    }

    #[test]
    fn test_end_offset_parts_falls_back_to_block_end() {
        let (prelude, expr) = end_offset_parts(&[], 0, "self.offset + 8");
        assert!(prelude.is_empty());
        assert_eq!(expr, "self.offset + 8");
    }

    #[test]
    fn test_var_data_getter_first_field_uses_block_end_when_no_groups() {
        let fields = [info("rawData", 4)];
        let code =
            generate_var_data_getter(0, &fields, 0, "self.offset + self.block_length as usize");
        assert!(code.contains("fn sbe_raw_data_offset(&self) -> usize {\n        self.offset + self.block_length as usize\n"));
        assert!(code.contains("pub fn raw_data(&self) -> &'a [u8]"));
        assert!(code.contains("let len = self.buffer.get_u32_le(pos) as usize;"));
    }

    #[test]
    fn test_var_data_getter_out_of_range_index_is_empty() {
        assert!(generate_var_data_getter(1, &[info("x", 2)], 0, "b").is_empty());
    }

    #[test]
    fn test_var_data_setter_uses_cursor_expression() {
        let code = generate_var_data_setter(&info("legTag", 2), "*self.limit", 0);
        assert!(code.contains("pub fn set_leg_tag(&mut self, value: &[u8]) -> &mut Self"));
        assert!(code.contains("self.buffer.put_u16_le(*self.limit, len);"));
        assert!(code.contains("let start = *self.limit + 2;"));
        assert!(code.contains("*self.limit = start + value.len();"));
    }

    #[test]
    fn test_var_data_setter_guards_its_part_index() {
        let code = generate_var_data_setter(&info("comment", 2), "self.limit", 3);
        assert!(code.contains("self.sbe_advance_to(3, \"var data field 'comment'\");"));
        assert!(code.contains("self.written = 4;"));
        // the guard runs before anything is written
        let guard = code.find("sbe_advance_to").expect("guard call");
        let write = code.find("put_u16_le").expect("header write");
        assert!(guard < write);
    }

    #[test]
    fn test_var_data_getter_uses_renamed_getter_but_plain_offset_helper() {
        let mut field = info("finish", 2);
        field.getter = "finish_".to_string();
        let code = generate_var_data_getter(0, std::slice::from_ref(&field), 0, "b");
        assert!(code.contains("fn sbe_finish_offset(&self) -> usize"));
        assert!(code.contains("pub fn finish_(&self) -> &'a [u8]"));
        assert!(code.contains("pub fn finish_as_str(&self) -> &'a str"));
        assert!(code.contains("Renamed from `finish` to avoid the generated `finish()` method."));
        assert!(!code.contains("pub fn finish(&self)"));
    }
}
