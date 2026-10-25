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
use crate::options::GeneratorOptions;
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
    /// `WriteBuffer` method that writes the length header.
    pub(crate) write_method: &'static str,
}

impl VarDataInfo {
    /// Human-readable part name used in generated error values, e.g.
    /// `var data field 'label'`.
    pub(crate) fn part_name(&self) -> String {
        format!("var data field '{}'", self.name)
    }
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

            let (header_length, write_method, length_rust_type, length_type) =
                match length_field.primitive_type {
                    PrimitiveType::Uint8 => (1, "put_u8", "u8", "uint8"),
                    PrimitiveType::Uint16 => (2, "put_u16_le", "u16", "uint16"),
                    PrimitiveType::Uint32 => (4, "put_u32_le", "u32", "uint32"),
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
    // previous field's header + payload, checked against the buffer.
    output.push_str(&format!(
        "    /// Byte offset of the length header of var data field `{}`.\n",
        info.name
    ));
    output.push_str("    #[inline]\n");
    output.push_str(&format!(
        "    fn sbe_{}_offset(&self) -> Result<usize, DecodeError> {{\n",
        info.accessor
    ));
    match index.checked_sub(1).and_then(|i| var_data.get(i)) {
        Some(prev) => {
            output.push_str(&format!(
                "        let (_, end) = ironsbe_core::bounds::var_data_at(self.buffer, self.sbe_{}_offset()?, {}, \"{}\")?;\n",
                prev.accessor,
                prev.header_length,
                prev.part_name()
            ));
            output.push_str("        Ok(end)\n");
        }
        None if group_count > 0 => {
            output.push_str(&format!("        self.sbe_group_offset({})\n", group_count));
        }
        None => {
            output.push_str(&format!("        Ok({block_end_expr})\n"));
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
    output.push_str("    /// # Errors\n");
    output.push_str(
        "    /// Returns [`DecodeError::VarPartOutOfBounds`] if this field, or a group or\n",
    );
    output.push_str("    /// var data field before it, extends past the end of the buffer.\n");
    output.push_str("    #[inline]\n");
    output.push_str(&format!(
        "    pub fn {}(&self) -> Result<&'a [u8], DecodeError> {{\n",
        info.getter
    ));
    output.push_str(&format!(
        "        let (value, _) = ironsbe_core::bounds::var_data_at(self.buffer, self.sbe_{}_offset()?, {}, \"{}\")?;\n",
        info.accessor,
        info.header_length,
        info.part_name()
    ));
    output.push_str("        Ok(value)\n");
    output.push_str("    }\n\n");

    // String accessor
    output.push_str(&format!(
        "    /// Var data field `{}` as UTF-8 (empty string if not valid UTF-8).\n",
        info.name
    ));
    output.push_str("    ///\n");
    output.push_str("    /// # Errors\n");
    output.push_str(&format!("    /// Same as [`Self::{}`].\n", info.getter));
    output.push_str("    #[inline]\n");
    output.push_str(&format!(
        "    pub fn {}_as_str(&self) -> Result<&'a str, DecodeError> {{\n",
        info.accessor
    ));
    output.push_str(&format!(
        "        Ok(std::str::from_utf8(self.{}()?).unwrap_or(\"\"))\n",
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
            format!(
                "        let (_, end) = ironsbe_core::bounds::var_data_at(self.buffer, self.sbe_{}_offset()?, {}, \"{}\")?;\n",
                last.accessor,
                last.header_length,
                last.part_name()
            ),
            "end".to_string(),
        ),
        None if group_count > 0 => (
            format!("        let end = self.sbe_group_offset({group_count})?;\n"),
            "end".to_string(),
        ),
        None => (String::new(), block_end_expr.to_string()),
    }
}

/// Generates `try_set_<name>` and, when `options` enable panicking
/// setters, `set_<name>` for one var data field on an encoder.
///
/// The host carries the variable-part guard (see `var_parts`): the setter
/// first fills every part skipped since the last write with an empty header,
/// then appends its own header and payload. `try_set_<name>` checks the
/// write order, the length header range and the buffer capacity before
/// writing anything; `set_<name>` wraps it and panics on error.
///
/// # Arguments
/// * `info` - Resolved field layout
/// * `cursor` - Place expression of the write cursor, e.g. `self.limit` on a
///   message encoder or `*self.limit` on an entry encoder
/// * `part_index` - Position of the field among the owner's variable parts
///   (repeating groups first, then var data fields)
/// * `options` - Generator options; decides whether `set_<name>` is emitted
/// * `encoder_methods` - Methods the owner's encoder already defines (its
///   group accessors, `<group>_count`); a `try_set_<name>` that would
///   duplicate one gets a trailing underscore instead
pub(crate) fn generate_var_data_setter(
    info: &VarDataInfo,
    cursor: &str,
    part_index: usize,
    options: GeneratorOptions,
    encoder_methods: &[String],
) -> String {
    let mut output = String::new();
    let name = &info.name;
    let accessor = &info.accessor;
    let len_type = info.length_rust_type;
    let plain_try_setter = format!("try_set_{accessor}");
    let try_setter = if encoder_methods.contains(&plain_try_setter) {
        format!("{plain_try_setter}_")
    } else {
        plain_try_setter.clone()
    };

    output.push_str(&format!(
        "    /// Set var data field: {} (id={}, length header: {}).\n",
        name, info.id, info.length_type
    ));
    output.push_str("    ///\n");
    output.push_str("    /// Appends the length header followed by `value` at the write cursor.\n");
    output.push_str("    /// Var data fields must be written in schema order, after all\n");
    output.push_str("    /// repeating groups. Any group or var data field skipped before this\n");
    output.push_str("    /// one is written as empty first; a field never written encodes as\n");
    output.push_str("    /// empty when the entry is dropped or the message is finished.\n");
    output.push_str("    ///\n");
    output.push_str("    /// # Errors\n");
    output.push_str(
        "    /// Returns [`ironsbe_core::error::EncodeError`] if this field or a later\n",
    );
    output.push_str("    /// part was already written, if `value.len()` does not fit in the\n");
    output.push_str("    /// length header, or if the buffer is too short. Nothing is written\n");
    output.push_str("    /// on error.\n");
    output.push_str("    ///\n");
    output.push_str("    /// The capacity check covers this field and the parts before it.\n");
    output.push_str("    /// Parts after it that are never written get an empty header on\n");
    output.push_str("    /// `finish()` or drop, which also needs buffer space: `finish()`\n");
    output.push_str("    /// returns an error if it does not fit.\n");
    if try_setter != plain_try_setter {
        output.push_str("    ///\n");
        output.push_str(&format!(
            "    /// Renamed from `{plain_try_setter}` to avoid the generated `{plain_try_setter}()` group accessor.\n"
        ));
    }
    output.push_str(&format!(
        "    pub fn {try_setter}(&mut self, value: &[u8]) -> Result<&mut Self, ironsbe_core::error::EncodeError> {{\n"
    ));
    let part = info.part_name();
    output.push_str(&format!(
        "        let Ok(len) = {len_type}::try_from(value.len()) else {{\n"
    ));
    output.push_str("            return Err(ironsbe_core::error::EncodeError::LengthOverflow {\n");
    output.push_str(&format!("                field: \"{part}\",\n"));
    output.push_str("                length: value.len(),\n");
    output.push_str(&format!(
        "                max: u64::from({len_type}::MAX),\n"
    ));
    output.push_str("            });\n");
    output.push_str("        };\n");
    output.push_str(&format!(
        "        let extra = {}usize.checked_add(value.len()).unwrap_or(usize::MAX);\n",
        info.header_length
    ));
    output.push_str(&format!(
        "        self.sbe_advance_to({part_index}, \"{part}\", extra)?;\n"
    ));
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
    output.push_str("        Ok(self)\n");
    output.push_str("    }\n\n");

    if options.panicking_setters() {
        output.push_str(&format!(
            "    /// Set var data field: {} (id={}, length header: {}).\n",
            name, info.id, info.length_type
        ));
        output.push_str("    ///\n");
        output.push_str(&format!(
            "    /// Panicking form of [`Self::{try_setter}`]; see it for the\n"
        ));
        output.push_str("    /// write rules.\n");
        output.push_str("    ///\n");
        output.push_str("    /// # Panics\n");
        output.push_str(
            "    /// Panics if `value.len()` does not fit in the length header, if the\n",
        );
        output.push_str(
            "    /// buffer is too short, or if this field or a later part was already\n",
        );
        output.push_str("    /// written (out-of-schema-order write).\n");
        output.push_str("    #[inline]\n");
        output.push_str(&format!(
            "    pub fn set_{accessor}(&mut self, value: &[u8]) -> &mut Self {{\n"
        ));
        output.push_str(&format!(
            "        if let Err(err) = self.{try_setter}(value) {{\n"
        ));
        output.push_str("            panic!(\"{err}\");\n");
        output.push_str("        }\n");
        output.push_str("        self\n");
        output.push_str("    }\n\n");
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(name: &str, header_length: usize) -> VarDataInfo {
        let (length_type, length_rust_type, write_method) = match header_length {
            1 => ("uint8", "u8", "put_u8"),
            4 => ("uint32", "u32", "put_u32_le"),
            _ => ("uint16", "u16", "put_u16_le"),
        };
        VarDataInfo {
            name: name.to_string(),
            getter: to_snake_case(name),
            accessor: to_snake_case(name),
            id: 1,
            length_type,
            length_rust_type,
            header_length,
            write_method,
        }
    }

    #[test]
    fn test_end_offset_parts_prefers_last_var_data() {
        let fields = [info("label", 2), info("payload", 1)];
        let (prelude, expr) = end_offset_parts(&fields, 3, "BLOCK_END");
        assert_eq!(
            prelude,
            "        let (_, end) = ironsbe_core::bounds::var_data_at(self.buffer, self.sbe_payload_offset()?, 1, \"var data field 'payload'\")?;\n"
        );
        assert_eq!(expr, "end");
    }

    #[test]
    fn test_end_offset_parts_falls_back_to_last_group() {
        let (prelude, expr) = end_offset_parts(&[], 2, "BLOCK_END");
        assert_eq!(prelude, "        let end = self.sbe_group_offset(2)?;\n");
        assert_eq!(expr, "end");
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
        assert!(code.contains("fn sbe_raw_data_offset(&self) -> Result<usize, DecodeError> {\n        Ok(self.offset + self.block_length as usize)\n"));
        assert!(code.contains("pub fn raw_data(&self) -> Result<&'a [u8], DecodeError>"));
        assert!(code.contains(
            "ironsbe_core::bounds::var_data_at(self.buffer, self.sbe_raw_data_offset()?, 4, \"var data field 'rawData'\")?;"
        ));
    }

    #[test]
    fn test_var_data_getter_out_of_range_index_is_empty() {
        assert!(generate_var_data_getter(1, &[info("x", 2)], 0, "b").is_empty());
    }

    #[test]
    fn test_var_data_setter_uses_cursor_expression() {
        let code = generate_var_data_setter(
            &info("legTag", 2),
            "*self.limit",
            0,
            GeneratorOptions::default(),
            &[],
        );
        assert!(code.contains("pub fn set_leg_tag(&mut self, value: &[u8]) -> &mut Self"));
        assert!(code.contains("self.buffer.put_u16_le(*self.limit, len);"));
        assert!(code.contains("let start = *self.limit + 2;"));
        assert!(code.contains("*self.limit = start + value.len();"));
    }

    #[test]
    fn test_var_data_setter_guards_its_part_index() {
        let code = generate_var_data_setter(
            &info("comment", 2),
            "self.limit",
            3,
            GeneratorOptions::default(),
            &[],
        );
        assert!(code.contains("self.sbe_advance_to(3, \"var data field 'comment'\", extra)?;"));
        assert!(code.contains("self.written = 4;"));
        // the guard runs before anything is written
        let guard = code.find("sbe_advance_to").expect("guard call");
        let write = code.find("put_u16_le").expect("header write");
        assert!(guard < write);
    }

    #[test]
    fn test_var_data_setter_try_set_checks_before_writing() {
        let code = generate_var_data_setter(
            &info("note", 1),
            "*self.limit",
            2,
            GeneratorOptions::default(),
            &[],
        );
        assert!(code.contains(
            "pub fn try_set_note(&mut self, value: &[u8]) -> Result<&mut Self, ironsbe_core::error::EncodeError>"
        ));
        assert!(code.contains("let Ok(len) = u8::try_from(value.len()) else {"));
        assert!(code.contains("max: u64::from(u8::MAX),"));
        // order and capacity (header width 1 + value) go through the guard
        assert!(
            code.contains("let extra = 1usize.checked_add(value.len()).unwrap_or(usize::MAX);")
        );
        assert!(code.contains("self.sbe_advance_to(2, \"var data field 'note'\", extra)?;"));
        // every check returns before the first write
        let try_set = code.find("pub fn try_set_note").expect("try_set");
        let last_check = code[try_set..]
            .find("sbe_advance_to")
            .expect("order and capacity check")
            + try_set;
        let write = code[try_set..].find("put_u8").expect("header write") + try_set;
        assert!(last_check < write);
    }

    #[test]
    fn test_var_data_setter_default_set_wraps_try_set() {
        let code = generate_var_data_setter(
            &info("note", 2),
            "self.limit",
            0,
            GeneratorOptions::default(),
            &[],
        );
        assert!(code.contains("pub fn set_note(&mut self, value: &[u8]) -> &mut Self"));
        assert!(code.contains("if let Err(err) = self.try_set_note(value) {"));
        assert!(code.contains("panic!(\"{err}\");"));
    }

    #[test]
    fn test_var_data_setter_without_panicking_setters_has_no_panic() {
        let options = GeneratorOptions::default().with_panicking_setters(false);
        let code = generate_var_data_setter(&info("note", 2), "self.limit", 0, options, &[]);
        assert!(code.contains("pub fn try_set_note("));
        assert!(!code.contains("pub fn set_note("));
        assert!(!code.contains("panic!"));
    }

    #[test]
    fn test_var_data_setter_try_set_renamed_when_group_accessor_collides() {
        let taken = vec!["try_set_order_count".to_string()];
        let code = generate_var_data_setter(
            &info("orderCount", 2),
            "self.limit",
            1,
            GeneratorOptions::default(),
            &taken,
        );
        assert!(code.contains("pub fn try_set_order_count_(&mut self, value: &[u8])"));
        assert!(!code.contains("pub fn try_set_order_count(&mut self"));
        assert!(code.contains("if let Err(err) = self.try_set_order_count_(value) {"));
        assert!(code.contains("[`Self::try_set_order_count_`]"));
        assert!(code.contains("Renamed from `try_set_order_count`"));
        // the panicking setter keeps its historical name
        assert!(code.contains("pub fn set_order_count(&mut self, value: &[u8]) -> &mut Self"));
    }

    #[test]
    fn test_var_data_getter_uses_renamed_getter_but_plain_offset_helper() {
        let mut field = info("finish", 2);
        field.getter = "finish_".to_string();
        let code = generate_var_data_getter(0, std::slice::from_ref(&field), 0, "b");
        assert!(code.contains("fn sbe_finish_offset(&self) -> Result<usize, DecodeError>"));
        assert!(code.contains("pub fn finish_(&self) -> Result<&'a [u8], DecodeError>"));
        assert!(code.contains("pub fn finish_as_str(&self) -> Result<&'a str, DecodeError>"));
        assert!(code.contains("Renamed from `finish` to avoid the generated `finish()` method."));
        assert!(!code.contains("pub fn finish(&self)"));
    }
}
