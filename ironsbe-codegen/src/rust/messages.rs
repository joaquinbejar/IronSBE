//! Message encoder/decoder code generation.

use ironsbe_schema::ir::{
    ResolvedField, ResolvedGroup, ResolvedMessage, SchemaIr, TypeKind, to_snake_case,
};
use ironsbe_schema::types::PrimitiveType;

use crate::error::CodegenError;

/// Resolved wire layout of one `<data>` (variable-length) field.
///
/// Built from the composite the field references (`varStringEncoding`,
/// `varDataEncoding`, ...): the `length` member decides how wide the length
/// header is on the wire and which buffer accessors read and write it.
struct VarDataInfo {
    /// Original schema name, used in doc comments.
    name: String,
    /// snake_case base name for the generated accessors.
    accessor: String,
    /// Field ID from the schema.
    id: u16,
    /// SBE name of the length primitive (`uint16`), used in doc comments.
    length_type: &'static str,
    /// Rust type of the length primitive (`u16`).
    length_rust_type: &'static str,
    /// Encoded width of the length header in bytes (1, 2 or 4).
    header_length: usize,
    /// `ReadBuffer` method that reads the length header.
    read_method: &'static str,
    /// `WriteBuffer` method that writes the length header.
    write_method: &'static str,
}

/// Generator for message encoders and decoders.
pub struct MessageGenerator<'a> {
    ir: &'a SchemaIr,
}

impl<'a> MessageGenerator<'a> {
    /// Creates a new message generator.
    #[must_use]
    pub fn new(ir: &'a SchemaIr) -> Self {
        Self { ir }
    }

    /// Generates all message definitions.
    ///
    /// # Errors
    /// Returns [`CodegenError::Unsupported`] for schema constructs the
    /// generator cannot emit correct code for, and
    /// [`CodegenError::UnknownType`] for `<data>` elements whose type is not
    /// declared in the schema.
    pub fn generate(&self) -> Result<String, CodegenError> {
        let mut output = String::new();

        for msg in &self.ir.messages {
            Self::validate_groups(msg)?;
            let var_data = self.resolve_var_data(msg)?;

            output.push_str(&self.generate_decoder(msg, &var_data));
            output.push_str(&self.generate_encoder(msg, &var_data));

            // Generate group decoders and encoders in a message-scoped module
            if !msg.groups.is_empty() {
                let mod_name = to_snake_case(&msg.name);
                output.push_str(&format!("/// Types for {} repeating groups.\n", msg.name));
                output.push_str(&format!("pub mod {} {{\n", mod_name));
                output.push_str("    use super::*;\n\n");
                for group in &msg.groups {
                    output.push_str(&self.generate_group_decoder(group));
                    output.push_str(&self.generate_group_encoder(group));
                }
                output.push_str("}\n\n");
            }
        }

        Ok(output)
    }

    /// Rejects group layouts the generated codecs cannot place correctly.
    ///
    /// Group entries are emitted with a fixed `blockLength` stride, so any
    /// `<data>` inside a group (at any depth) cannot be positioned. A message
    /// that carries var data after a group with nested groups has the same
    /// problem: the var data offset walk assumes flat entries.
    fn validate_groups(msg: &ResolvedMessage) -> Result<(), CodegenError> {
        if let Some(group) = Self::find_group_with_var_data(&msg.groups) {
            return Err(CodegenError::unsupported(
                "<data> inside repeating group",
                format!("message '{}', group '{}'", msg.name, group.name),
            ));
        }

        if !msg.var_data.is_empty()
            && let Some(group) = msg.groups.iter().find(|g| !g.nested_groups.is_empty())
        {
            return Err(CodegenError::unsupported(
                "<data> after a repeating group with nested groups",
                format!("message '{}', group '{}'", msg.name, group.name),
            ));
        }

        Ok(())
    }

    /// Depth-first search for a group that declares `<data>` fields.
    fn find_group_with_var_data(groups: &[ResolvedGroup]) -> Option<&ResolvedGroup> {
        groups.iter().find_map(|g| {
            if g.var_data.is_empty() {
                Self::find_group_with_var_data(&g.nested_groups)
            } else {
                Some(g)
            }
        })
    }

    /// Resolves the length-header layout of every `<data>` field in `msg`.
    fn resolve_var_data(&self, msg: &ResolvedMessage) -> Result<Vec<VarDataInfo>, CodegenError> {
        msg.var_data
            .iter()
            .map(|vd| {
                let context = format!("message '{}', data '{}'", msg.name, vd.name);
                let field_path = format!("{}.{}", msg.name, vd.name);

                let resolved = self
                    .ir
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

    /// Generates a message decoder.
    fn generate_decoder(&self, msg: &ResolvedMessage, var_data: &[VarDataInfo]) -> String {
        let mut output = String::new();
        let decoder_name = msg.decoder_name();

        // Struct definition
        output.push_str(&format!("/// {} Decoder (zero-copy).\n", msg.name));
        output.push_str("#[derive(Debug, Clone, Copy)]\n");
        output.push_str(&format!("pub struct {}<'a> {{\n", decoder_name));
        output.push_str("    buffer: &'a [u8],\n");
        output.push_str("    offset: usize,\n");
        output.push_str("    acting_version: u16,\n");
        output.push_str("}\n\n");

        // Implementation
        output.push_str(&format!("impl<'a> {}<'a> {{\n", decoder_name));
        output.push_str(&format!(
            "    /// Template ID for this message.\n\
             pub const TEMPLATE_ID: u16 = {};\n",
            msg.template_id
        ));
        output.push_str(&format!(
            "    /// Block length of the fixed portion.\n\
             pub const BLOCK_LENGTH: u16 = {};\n\n",
            msg.block_length
        ));

        // Constructor
        output.push_str("    /// Wraps a buffer for zero-copy decoding.\n");
        output.push_str("    ///\n");
        output.push_str("    /// # Arguments\n");
        output.push_str("    /// * `buffer` - Buffer containing the message\n");
        output.push_str(
            "    /// * `offset` - Offset to the start of the root block (after header)\n",
        );
        output.push_str("    /// * `acting_version` - Schema version for compatibility\n");
        output.push_str("    #[inline]\n");
        output.push_str("    #[must_use]\n");
        output.push_str(
            "    pub fn wrap(buffer: &'a [u8], offset: usize, acting_version: u16) -> Self {\n",
        );
        output.push_str("        Self { buffer, offset, acting_version }\n");
        output.push_str("    }\n\n");

        // Field getters
        for field in &msg.fields {
            output.push_str(&self.generate_field_getter(field));
        }

        // Group accessors. Groups follow the fixed block back to back, so the
        // offset of group `i` depends on the entry counts of groups `0..i`
        // and has to be walked on the wire.
        if !msg.groups.is_empty() {
            output.push_str(&Self::generate_group_offset_helper());
        }
        for (index, group) in msg.groups.iter().enumerate() {
            output.push_str(&Self::generate_group_accessor(group, index, &msg.name));
        }

        // Var data accessors (after all groups, in schema order)
        for index in 0..var_data.len() {
            output.push_str(&Self::generate_var_data_getter(
                index,
                var_data,
                msg.groups.len(),
            ));
        }

        output.push_str("}\n\n");

        // SbeDecoder trait implementation
        output.push_str(&format!(
            "impl<'a> SbeDecoder<'a> for {}<'a> {{\n",
            decoder_name
        ));
        output.push_str(&format!(
            "    const TEMPLATE_ID: u16 = {};\n",
            msg.template_id
        ));
        output.push_str("    const SCHEMA_ID: u16 = SCHEMA_ID;\n");
        output.push_str("    const SCHEMA_VERSION: u16 = SCHEMA_VERSION;\n");
        output.push_str(&format!(
            "    const BLOCK_LENGTH: u16 = {};\n\n",
            msg.block_length
        ));

        output.push_str(
            "    fn wrap(buffer: &'a [u8], offset: usize, acting_version: u16) -> Self {\n",
        );
        output.push_str("        Self::wrap(buffer, offset, acting_version)\n");
        output.push_str("    }\n\n");

        output.push_str(&Self::generate_decoder_encoded_length(
            var_data,
            msg.groups.len(),
        ));
        output.push_str("}\n\n");

        output
    }

    /// Generates the private `group_offset(index)` walker on a message decoder.
    ///
    /// Shared by the group accessors, the var data accessors and
    /// `encoded_length`.
    fn generate_group_offset_helper() -> String {
        let mut output = String::new();

        output.push_str(
            "    /// Byte offset of the header of the `index`-th repeating group (0-based).\n",
        );
        output.push_str("    ///\n");
        output.push_str("    /// Walks the preceding group headers on the wire, so the cost is\n");
        output.push_str(
            "    /// O(`index`) header reads. Entries are exactly `blockLength` bytes.\n",
        );
        output.push_str("    #[inline]\n");
        output.push_str("    fn group_offset(&self, index: usize) -> usize {\n");
        output.push_str("        let mut pos = self.offset + Self::BLOCK_LENGTH as usize;\n");
        output.push_str("        for _ in 0..index {\n");
        output.push_str("            pos += GroupHeader::wrap(self.buffer, pos).group_size();\n");
        output.push_str("        }\n");
        output.push_str("        pos\n");
        output.push_str("    }\n\n");

        output
    }

    /// Generates the private `<name>_offset()` helper plus the public slice
    /// and string accessors for the `index`-th var data field of a message.
    fn generate_var_data_getter(
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
        output.push_str(
            "    /// Returns the raw bytes. Var data fields follow all repeating groups\n",
        );
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
    fn generate_decoder_encoded_length(var_data: &[VarDataInfo], group_count: usize) -> String {
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
                output.push_str(
                    "        MessageHeader::ENCODED_LENGTH + Self::BLOCK_LENGTH as usize\n",
                );
            }
        }
        output.push_str("    }\n");

        output
    }

    /// Generates a field getter method.
    fn generate_field_getter(&self, field: &ResolvedField) -> String {
        let mut output = String::new();

        output.push_str(&format!(
            "    /// Field: {} (id={}, offset={}).\n",
            field.name, field.id, field.offset
        ));
        output.push_str("    #[inline(always)]\n");
        output.push_str("    #[must_use]\n");

        if field.is_array {
            // Array field - return slice
            let elem_type = field.primitive_type.map(|p| p.rust_type()).unwrap_or("u8");
            let len = field.array_length.unwrap_or(1);

            if elem_type == "u8" {
                // Byte array - return &[u8]
                output.push_str(&format!(
                    "    pub fn {}(&self) -> &'a [u8] {{\n",
                    field.getter_name
                ));
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
                    field.getter_name
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
                output.push_str(&format!(
                    "    pub fn {}(&self) -> &'a [u8] {{\n",
                    field.getter_name
                ));
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
            let resolved_type = self.ir.get_type(&field.type_name);

            match resolved_type.map(|t| &t.kind) {
                Some(TypeKind::Enum { encoding, .. }) => {
                    // Enum field - use encoding primitive and wrap with From
                    let read_method = get_read_method(Some(*encoding));
                    output.push_str(&format!(
                        "    pub fn {}(&self) -> {} {{\n",
                        field.getter_name, rust_type
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
                        field.getter_name, rust_type
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
                        field.getter_name, rust_type
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
                        field.getter_name, rust_type
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

    /// Generates a group accessor method.
    ///
    /// `index` is the 0-based position of the group in the message; the
    /// accessor resolves its byte offset through `group_offset(index)`.
    fn generate_group_accessor(group: &ResolvedGroup, index: usize, msg_name: &str) -> String {
        let mut output = String::new();
        let qualified = format!("{}::{}", to_snake_case(msg_name), group.decoder_name());

        output.push_str(&format!("    /// Access {} repeating group.\n", group.name));
        output.push_str("    #[inline]\n");
        output.push_str("    #[must_use]\n");
        output.push_str(&format!(
            "    pub fn {}(&self) -> {}<'a> {{\n",
            to_snake_case(&group.name),
            qualified
        ));
        output.push_str(&format!(
            "        {}::wrap(self.buffer, self.group_offset({}))\n",
            qualified, index
        ));
        output.push_str("    }\n\n");

        output
    }

    /// Generates a message encoder.
    fn generate_encoder(&self, msg: &ResolvedMessage, var_data: &[VarDataInfo]) -> String {
        let mut output = String::new();
        let encoder_name = msg.encoder_name();

        // Struct definition
        output.push_str(&format!("/// {} Encoder.\n", msg.name));
        output.push_str("///\n");
        output.push_str("/// Fixed fields are written at their schema offsets. Repeating groups\n");
        output.push_str("/// and var data fields are appended at a write cursor (`limit`) and\n");
        output.push_str("/// must be written in schema order.\n");
        output.push_str(&format!("pub struct {}<'a> {{\n", encoder_name));
        output.push_str("    buffer: &'a mut [u8],\n");
        output.push_str("    offset: usize,\n");
        output.push_str("    limit: usize,\n");
        output.push_str("}\n\n");

        // Implementation
        output.push_str(&format!("impl<'a> {}<'a> {{\n", encoder_name));
        output.push_str(&format!(
            "    /// Template ID for this message.\n\
             pub const TEMPLATE_ID: u16 = {};\n",
            msg.template_id
        ));
        output.push_str(&format!(
            "    /// Block length of the fixed portion.\n\
             pub const BLOCK_LENGTH: u16 = {};\n\n",
            msg.block_length
        ));

        // Constructor
        output.push_str("    /// Wraps a buffer for encoding, writing the header.\n");
        output.push_str("    ///\n");
        output.push_str("    /// The write cursor starts right after the fixed block.\n");
        output.push_str("    #[inline]\n");
        output.push_str("    pub fn wrap(buffer: &'a mut [u8], offset: usize) -> Self {\n");
        output.push_str(
            "        let limit = offset + MessageHeader::ENCODED_LENGTH + Self::BLOCK_LENGTH as usize;\n",
        );
        output.push_str("        let mut encoder = Self { buffer, offset, limit };\n");
        output.push_str("        encoder.write_header();\n");
        output.push_str("        encoder\n");
        output.push_str("    }\n\n");

        // Write header
        output.push_str("    fn write_header(&mut self) {\n");
        output.push_str("        let header = MessageHeader {\n");
        output.push_str("            block_length: Self::BLOCK_LENGTH,\n");
        output.push_str("            template_id: Self::TEMPLATE_ID,\n");
        output.push_str("            schema_id: SCHEMA_ID,\n");
        output.push_str("            version: SCHEMA_VERSION,\n");
        output.push_str("        };\n");
        output.push_str("        header.encode(self.buffer, self.offset);\n");
        output.push_str("    }\n\n");

        // Encoded length
        output.push_str("    /// Returns the encoded length of the message so far: header,\n");
        output.push_str("    /// fixed block, and every repeating group and var data field\n");
        output.push_str("    /// written through this encoder.\n");
        output.push_str("    #[inline]\n");
        output.push_str("    #[must_use]\n");
        output.push_str("    pub const fn encoded_length(&self) -> usize {\n");
        output.push_str("        self.limit - self.offset\n");
        output.push_str("    }\n\n");

        // Field setters
        for field in &msg.fields {
            output.push_str(&self.generate_field_setter(field));
        }

        // Group encoder accessors (advance the write cursor)
        for group in &msg.groups {
            output.push_str(&Self::generate_group_encoder_accessor(group, &msg.name));
        }

        // Var data setters (append at the write cursor)
        for info in var_data {
            output.push_str(&Self::generate_var_data_setter(info));
        }

        output.push_str("}\n\n");

        output
    }

    /// Generates `set_<name>` for one var data field on a message encoder.
    fn generate_var_data_setter(info: &VarDataInfo) -> String {
        let mut output = String::new();

        output.push_str(&format!(
            "    /// Set var data field: {} (id={}, length header: {}).\n",
            info.name, info.id, info.length_type
        ));
        output.push_str("    ///\n");
        output.push_str(
            "    /// Appends the length header followed by `value` at the write cursor.\n",
        );
        output.push_str("    /// Var data fields must be written in schema order, after all\n");
        output.push_str("    /// repeating groups.\n");
        output.push_str("    ///\n");
        output.push_str("    /// # Panics\n");
        output
            .push_str("    /// Panics if `value.len()` does not fit in the length header, or if\n");
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

    /// Generates a field setter method.
    fn generate_field_setter(&self, field: &ResolvedField) -> String {
        let mut output = String::new();
        let field_offset = format!("MessageHeader::ENCODED_LENGTH + {}", field.offset);

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
            let resolved_type = self.ir.get_type(&field.type_name);

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

    /// Generates a group decoder.
    fn generate_group_decoder(&self, group: &ResolvedGroup) -> String {
        let mut output = String::new();
        let decoder_name = group.decoder_name();
        let entry_name = group.entry_decoder_name();

        // Group decoder struct
        output.push_str(&format!("/// {} Group Decoder.\n", group.name));
        output.push_str("#[derive(Debug, Clone, Copy)]\n");
        output.push_str(&format!("pub struct {}<'a> {{\n", decoder_name));
        output.push_str("    buffer: &'a [u8],\n");
        output.push_str("    block_length: u16,\n");
        output.push_str("    count: u16,\n");
        output.push_str("    index: u16,\n");
        output.push_str("    offset: usize,\n");
        output.push_str("}\n\n");

        // Group decoder implementation
        output.push_str(&format!("impl<'a> {}<'a> {{\n", decoder_name));
        output.push_str("    /// Wraps a buffer at the group header position.\n");
        output.push_str("    #[must_use]\n");
        output.push_str("    pub fn wrap(buffer: &'a [u8], offset: usize) -> Self {\n");
        output.push_str("        let header = GroupHeader::wrap(buffer, offset);\n");
        output.push_str("        Self {\n");
        output.push_str("            buffer,\n");
        output.push_str("            block_length: header.block_length,\n");
        output.push_str("            count: header.num_in_group,\n");
        output.push_str("            index: 0,\n");
        output.push_str("            offset: offset + GroupHeader::ENCODED_LENGTH,\n");
        output.push_str("        }\n");
        output.push_str("    }\n\n");

        output.push_str("    /// Returns the number of entries in the group.\n");
        output.push_str("    #[must_use]\n");
        output.push_str("    pub const fn count(&self) -> u16 {\n");
        output.push_str("        self.count\n");
        output.push_str("    }\n\n");

        output.push_str("    /// Returns true if the group is empty.\n");
        output.push_str("    #[must_use]\n");
        output.push_str("    pub const fn is_empty(&self) -> bool {\n");
        output.push_str("        self.count == 0\n");
        output.push_str("    }\n");
        output.push_str("}\n\n");

        // Iterator implementation
        output.push_str(&format!("impl<'a> Iterator for {}<'a> {{\n", decoder_name));
        output.push_str(&format!("    type Item = {}<'a>;\n\n", entry_name));
        output.push_str("    fn next(&mut self) -> Option<Self::Item> {\n");
        output.push_str("        if self.index >= self.count {\n");
        output.push_str("            return None;\n");
        output.push_str("        }\n");
        output.push_str(&format!(
            "        let entry = {}::wrap(self.buffer, self.offset);\n",
            entry_name
        ));
        output.push_str("        self.offset += self.block_length as usize;\n");
        output.push_str("        self.index += 1;\n");
        output.push_str("        Some(entry)\n");
        output.push_str("    }\n\n");

        output.push_str("    fn size_hint(&self) -> (usize, Option<usize>) {\n");
        output.push_str("        let remaining = (self.count - self.index) as usize;\n");
        output.push_str("        (remaining, Some(remaining))\n");
        output.push_str("    }\n");
        output.push_str("}\n\n");

        output.push_str(&format!(
            "impl<'a> ExactSizeIterator for {}<'a> {{}}\n\n",
            decoder_name
        ));

        // Entry decoder
        output.push_str(&self.generate_entry_decoder(group));

        // Nested groups
        for nested in &group.nested_groups {
            output.push_str(&self.generate_group_decoder(nested));
        }

        output
    }

    /// Generates a group entry decoder.
    fn generate_entry_decoder(&self, group: &ResolvedGroup) -> String {
        let mut output = String::new();
        let entry_name = group.entry_decoder_name();

        output.push_str(&format!("/// {} Entry Decoder.\n", group.name));
        output.push_str("#[derive(Debug, Clone, Copy)]\n");
        output.push_str(&format!("pub struct {}<'a> {{\n", entry_name));
        output.push_str("    buffer: &'a [u8],\n");
        output.push_str("    offset: usize,\n");
        output.push_str("}\n\n");

        output.push_str(&format!("impl<'a> {}<'a> {{\n", entry_name));
        output.push_str("    fn wrap(buffer: &'a [u8], offset: usize) -> Self {\n");
        output.push_str("        Self { buffer, offset }\n");
        output.push_str("    }\n\n");

        // Field getters
        for field in &group.fields {
            output.push_str(&self.generate_field_getter(field));
        }

        output.push_str("}\n\n");

        output
    }

    /// Generates a group encoder.
    fn generate_group_encoder(&self, group: &ResolvedGroup) -> String {
        let mut output = String::new();
        let encoder_name = group.encoder_name();
        let entry_name = group.entry_encoder_name();

        // Compute effective block length: use XML value if nonzero, else derive from fields
        let effective_block_length = if group.block_length > 0 {
            group.block_length
        } else {
            group
                .fields
                .iter()
                .map(|f| f.offset + f.encoded_length)
                .max()
                .unwrap_or(0) as u16
        };

        // Group encoder struct
        output.push_str(&format!("/// {} Group Encoder.\n", group.name));
        output.push_str(&format!("pub struct {}<'a> {{\n", encoder_name));
        output.push_str("    buffer: &'a mut [u8],\n");
        output.push_str("    count: u16,\n");
        output.push_str("    index: u16,\n");
        output.push_str("    offset: usize,\n");
        output.push_str("}\n\n");

        // Group encoder implementation
        output.push_str(&format!("impl<'a> {}<'a> {{\n", encoder_name));
        output.push_str(&format!(
            "    /// Block length of each entry.\n\
             pub const BLOCK_LENGTH: u16 = {};\n\n",
            effective_block_length
        ));

        // wrap constructor
        output
            .push_str("    /// Wraps a buffer at the group header position, writing the header.\n");
        output.push_str("    ///\n");
        output.push_str("    /// # Arguments\n");
        output.push_str("    /// * `buffer` - Mutable buffer to write to\n");
        output.push_str("    /// * `offset` - Offset of the group header\n");
        output.push_str("    /// * `count` - Number of entries to encode\n");
        output.push_str(
            "    pub fn wrap(buffer: &'a mut [u8], offset: usize, count: u16) -> Self {\n",
        );
        output.push_str("        let header = GroupHeader::new(Self::BLOCK_LENGTH, count);\n");
        output.push_str("        header.encode(buffer, offset);\n");
        output.push_str("        Self {\n");
        output.push_str("            buffer,\n");
        output.push_str("            count,\n");
        output.push_str("            index: 0,\n");
        output.push_str("            offset: offset + GroupHeader::ENCODED_LENGTH,\n");
        output.push_str("        }\n");
        output.push_str("    }\n\n");

        // next_entry
        output.push_str(
            "    /// Returns the next entry encoder, or `None` if all entries are written.\n",
        );
        output.push_str(&format!(
            "    pub fn next_entry(&mut self) -> Option<{}<'_>> {{\n",
            entry_name
        ));
        output.push_str("        if self.index >= self.count {\n");
        output.push_str("            return None;\n");
        output.push_str("        }\n");
        output.push_str("        let offset = self.offset;\n");
        output.push_str("        self.offset += Self::BLOCK_LENGTH as usize;\n");
        output.push_str("        self.index += 1;\n");
        output.push_str(&format!(
            "        Some({}::wrap(&mut *self.buffer, offset))\n",
            entry_name
        ));
        output.push_str("    }\n\n");

        // encoded_length
        output.push_str(
            "    /// Returns the total encoded length of this group (header + all entries).\n",
        );
        output.push_str("    #[must_use]\n");
        output.push_str("    pub const fn encoded_length(&self) -> usize {\n");
        output.push_str("        GroupHeader::ENCODED_LENGTH + Self::BLOCK_LENGTH as usize * self.count as usize\n");
        output.push_str("    }\n");
        output.push_str("}\n\n");

        // Entry encoder
        output.push_str(&self.generate_entry_encoder(group));

        // Nested group encoders
        for nested in &group.nested_groups {
            output.push_str(&self.generate_group_encoder(nested));
        }

        output
    }

    /// Generates a group entry encoder.
    fn generate_entry_encoder(&self, group: &ResolvedGroup) -> String {
        let mut output = String::new();
        let entry_name = group.entry_encoder_name();

        output.push_str(&format!("/// {} Entry Encoder.\n", group.name));
        output.push_str(&format!("pub struct {}<'a> {{\n", entry_name));
        output.push_str("    buffer: &'a mut [u8],\n");
        output.push_str("    offset: usize,\n");
        output.push_str("}\n\n");

        output.push_str(&format!("impl<'a> {}<'a> {{\n", entry_name));
        output.push_str("    pub fn wrap(buffer: &'a mut [u8], offset: usize) -> Self {\n");
        output.push_str("        Self { buffer, offset }\n");
        output.push_str("    }\n\n");

        // Field setters
        for field in &group.fields {
            output.push_str(&self.generate_entry_field_setter(field));
        }

        output.push_str("}\n\n");

        output
    }

    /// Generates a field setter for a group entry encoder.
    ///
    /// Unlike the message-level `generate_field_setter`, this uses the raw field
    /// offset (relative to the entry start) without a `MessageHeader::ENCODED_LENGTH`
    /// prefix.
    fn generate_entry_field_setter(&self, field: &ResolvedField) -> String {
        let mut output = String::new();
        let field_offset = field.offset;

        output.push_str(&format!(
            "    /// Set field: {} (id={}, offset={}).\n",
            field.name, field.id, field.offset
        ));
        output.push_str("    #[inline(always)]\n");

        if field.is_array {
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
            let rust_type = &field.rust_type;
            let resolved_type = self.ir.get_type(&field.type_name);

            match resolved_type.map(|t| &t.kind) {
                Some(TypeKind::Enum { encoding, .. }) => {
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

    /// Generates a group encoder accessor on the parent message encoder.
    ///
    /// The group is placed at the current write cursor, and the cursor is
    /// advanced past the header and `count` entries so the next group or var
    /// data field lands right after it.
    fn generate_group_encoder_accessor(group: &ResolvedGroup, msg_name: &str) -> String {
        let mut output = String::new();
        let qualified = format!("{}::{}", to_snake_case(msg_name), group.encoder_name());

        output.push_str(&format!(
            "    /// Begin encoding the {} repeating group.\n",
            group.name
        ));
        output.push_str("    ///\n");
        output.push_str(
            "    /// Advances the write cursor past the group header and `count` entries.\n",
        );
        output.push_str(&format!(
            "    pub fn {}_count(&mut self, count: u16) -> {}<'_> {{\n",
            to_snake_case(&group.name),
            qualified
        ));
        output.push_str("        let offset = self.limit;\n");
        output.push_str(&format!(
            "        self.limit += GroupHeader::ENCODED_LENGTH + {}::BLOCK_LENGTH as usize * count as usize;\n",
            qualified
        ));
        output.push_str(&format!(
            "        {}::wrap(&mut *self.buffer, offset, count)\n",
            qualified
        ));
        output.push_str("    }\n\n");

        output
    }
}

/// Gets the read method name for a primitive type.
fn get_read_method(prim: Option<PrimitiveType>) -> &'static str {
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

/// Gets the write method name for a primitive type.
fn get_write_method(prim: Option<PrimitiveType>) -> &'static str {
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
    use ironsbe_schema::{SchemaIr, parse_schema};

    fn schema_with_shared_group_name() -> String {
        r#"<?xml version="1.0" encoding="UTF-8"?>
<sbe:messageSchema xmlns:sbe="http://fixprotocol.io/2016/sbe"
                   package="test" id="1" version="1" byteOrder="littleEndian">
    <types>
        <type name="uint64" primitiveType="uint64"/>
    </types>
    <sbe:message name="CreateRfqResponse" id="21" blockLength="8">
        <field name="value" id="1" type="uint64" offset="0"/>
        <group name="quotes" id="100" dimensionType="groupSizeEncoding" blockLength="8">
            <field name="price" id="200" type="uint64" offset="0"/>
        </group>
    </sbe:message>
    <sbe:message name="GetRfqResponse" id="23" blockLength="8">
        <field name="value" id="1" type="uint64" offset="0"/>
        <group name="quotes" id="100" dimensionType="groupSizeEncoding" blockLength="8">
            <field name="price" id="200" type="uint64" offset="0"/>
        </group>
    </sbe:message>
</sbe:messageSchema>"#
            .to_string()
    }

    fn schema_with_group_no_offsets() -> String {
        r#"<?xml version="1.0" encoding="UTF-8"?>
<sbe:messageSchema xmlns:sbe="http://fixprotocol.io/2016/sbe"
                   package="test" id="1" version="1" byteOrder="littleEndian">
    <types>
        <type name="uint64" primitiveType="uint64"/>
        <type name="uint32" primitiveType="uint32"/>
    </types>
    <sbe:message name="ListOrders" id="19" blockLength="0">
        <group name="orders" id="100" dimensionType="groupSizeEncoding" blockLength="20">
            <field name="orderId" id="1" type="uint64" offset="0"/>
            <field name="instrumentId" id="2" type="uint32"/>
            <field name="quantity" id="3" type="uint64"/>
        </group>
    </sbe:message>
</sbe:messageSchema>"#
            .to_string()
    }

    fn schema_with_group_explicit_offsets() -> String {
        r#"<?xml version="1.0" encoding="UTF-8"?>
<sbe:messageSchema xmlns:sbe="http://fixprotocol.io/2016/sbe"
                   package="test" id="1" version="1" byteOrder="littleEndian">
    <types>
        <type name="uint64" primitiveType="uint64"/>
        <type name="uint32" primitiveType="uint32"/>
    </types>
    <sbe:message name="ListOrders" id="19" blockLength="0">
        <group name="orders" id="100" dimensionType="groupSizeEncoding" blockLength="20">
            <field name="orderId" id="1" type="uint64" offset="0"/>
            <field name="instrumentId" id="2" type="uint32" offset="8"/>
            <field name="quantity" id="3" type="uint64" offset="12"/>
        </group>
    </sbe:message>
</sbe:messageSchema>"#
            .to_string()
    }

    #[test]
    fn test_duplicate_group_name_generates_scoped_modules() {
        let xml = schema_with_shared_group_name();
        let schema = parse_schema(&xml).expect("Failed to parse schema");
        let ir = SchemaIr::from_schema(&schema);
        let msg_gen = MessageGenerator::new(&ir);
        let code = msg_gen.generate().expect("codegen failed");

        assert!(
            code.contains("pub mod create_rfq_response {"),
            "expected module for CreateRfqResponse groups"
        );
        assert!(
            code.contains("pub mod get_rfq_response {"),
            "expected module for GetRfqResponse groups"
        );

        let occurrences = code.matches("pub struct QuotesGroupDecoder").count();
        assert_eq!(
            occurrences, 2,
            "expected one QuotesGroupDecoder per message module, got {occurrences}"
        );
    }

    #[test]
    fn test_group_accessor_uses_qualified_path() {
        let xml = schema_with_shared_group_name();
        let schema = parse_schema(&xml).expect("Failed to parse schema");
        let ir = SchemaIr::from_schema(&schema);
        let msg_gen = MessageGenerator::new(&ir);
        let code = msg_gen.generate().expect("codegen failed");

        assert!(
            code.contains("create_rfq_response::QuotesGroupDecoder"),
            "accessor in CreateRfqResponse must reference module-qualified type"
        );
        assert!(
            code.contains("get_rfq_response::QuotesGroupDecoder"),
            "accessor in GetRfqResponse must reference module-qualified type"
        );
    }

    #[test]
    fn test_entry_decoder_field_offsets_auto_computed() {
        let xml = schema_with_group_no_offsets();
        let schema = parse_schema(&xml).expect("Failed to parse schema");
        let ir = SchemaIr::from_schema(&schema);
        let msg_gen = MessageGenerator::new(&ir);
        let code = msg_gen.generate().expect("codegen failed");

        // orderId at offset 0
        assert!(
            code.contains("self.offset + 0)"),
            "orderId should be at offset 0"
        );
        // instrumentId at offset 8 (after uint64)
        assert!(
            code.contains("self.offset + 8)"),
            "instrumentId should be at offset 8, not 0"
        );
        // quantity at offset 12 (after uint64 + uint32)
        assert!(
            code.contains("self.offset + 12)"),
            "quantity should be at offset 12, not 0"
        );
    }

    #[test]
    fn test_entry_decoder_field_offsets_explicit() {
        let xml = schema_with_group_explicit_offsets();
        let schema = parse_schema(&xml).expect("Failed to parse schema");
        let ir = SchemaIr::from_schema(&schema);
        let msg_gen = MessageGenerator::new(&ir);
        let code = msg_gen.generate().expect("codegen failed");

        assert!(
            code.contains("self.offset + 8)"),
            "instrumentId should be at explicit offset 8"
        );
        assert!(
            code.contains("self.offset + 12)"),
            "quantity should be at explicit offset 12"
        );
    }

    #[test]
    fn test_group_encoder_emitted() {
        let xml = schema_with_group_no_offsets();
        let schema = parse_schema(&xml).expect("Failed to parse schema");
        let ir = SchemaIr::from_schema(&schema);
        let msg_gen = MessageGenerator::new(&ir);
        let code = msg_gen.generate().expect("codegen failed");

        assert!(
            code.contains("pub struct OrdersGroupEncoder"),
            "expected OrdersGroupEncoder struct"
        );
        assert!(
            code.contains("pub struct OrdersEntryEncoder"),
            "expected OrdersEntryEncoder struct"
        );
    }

    #[test]
    fn test_group_encoder_has_next_entry() {
        let xml = schema_with_group_no_offsets();
        let schema = parse_schema(&xml).expect("Failed to parse schema");
        let ir = SchemaIr::from_schema(&schema);
        let msg_gen = MessageGenerator::new(&ir);
        let code = msg_gen.generate().expect("codegen failed");

        assert!(
            code.contains("fn next_entry(&mut self)"),
            "expected next_entry method on group encoder"
        );
    }

    #[test]
    fn test_entry_encoder_has_field_setters() {
        let xml = schema_with_group_no_offsets();
        let schema = parse_schema(&xml).expect("Failed to parse schema");
        let ir = SchemaIr::from_schema(&schema);
        let msg_gen = MessageGenerator::new(&ir);
        let code = msg_gen.generate().expect("codegen failed");

        assert!(
            code.contains("fn set_order_id(&mut self, value: u64)"),
            "expected set_order_id setter"
        );
        assert!(
            code.contains("fn set_instrument_id(&mut self, value: u32)"),
            "expected set_instrument_id setter"
        );
        assert!(
            code.contains("fn set_quantity(&mut self, value: u64)"),
            "expected set_quantity setter"
        );
    }

    #[test]
    fn test_parent_encoder_has_group_accessor() {
        let xml = schema_with_group_no_offsets();
        let schema = parse_schema(&xml).expect("Failed to parse schema");
        let ir = SchemaIr::from_schema(&schema);
        let msg_gen = MessageGenerator::new(&ir);
        let code = msg_gen.generate().expect("codegen failed");

        assert!(
            code.contains("fn orders_count(&mut self, count: u16)"),
            "expected orders_count accessor on parent encoder"
        );
    }

    #[test]
    fn test_roundtrip_group_codegen_structure() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<sbe:messageSchema xmlns:sbe="http://fixprotocol.io/2016/sbe"
                   package="test" id="1" version="1" byteOrder="littleEndian">
    <types>
        <type name="uint64" primitiveType="uint64"/>
        <type name="uint32" primitiveType="uint32"/>
        <type name="uint8" primitiveType="uint8"/>
    </types>
    <sbe:message name="ListOrders" id="19" blockLength="8">
        <field name="requestId" id="1" type="uint64" offset="0"/>
        <group name="orders" id="100" dimensionType="groupSizeEncoding" blockLength="29">
            <field name="orderId" id="10" type="uint64" offset="0"/>
            <field name="instrumentId" id="11" type="uint32"/>
            <field name="price" id="12" type="uint64"/>
            <field name="quantity" id="13" type="uint64"/>
            <field name="side" id="14" type="uint8"/>
        </group>
    </sbe:message>
</sbe:messageSchema>"#;

        let schema = parse_schema(xml).expect("Failed to parse schema");
        let ir = SchemaIr::from_schema(&schema);
        let msg_gen = MessageGenerator::new(&ir);
        let code = msg_gen.generate().expect("codegen failed");

        // --- Decoder side ---
        let decoder_pos = code
            .find("impl<'a> OrdersEntryDecoder<'a>")
            .expect("entry decoder impl");
        let decoder_section = &code[decoder_pos..];
        // Verify all five fields have distinct offsets
        assert!(decoder_section.contains("self.offset + 0)"));
        assert!(decoder_section.contains("self.offset + 8)"));
        assert!(decoder_section.contains("self.offset + 12)"));
        assert!(decoder_section.contains("self.offset + 20)"));
        assert!(decoder_section.contains("self.offset + 28)"));

        // --- Encoder side ---
        let encoder_pos = code
            .find("impl<'a> OrdersEntryEncoder<'a>")
            .expect("entry encoder impl");
        let encoder_section = &code[encoder_pos..];
        // Verify setter offsets match decoder offsets
        assert!(encoder_section.contains("self.offset + 0,"));
        assert!(encoder_section.contains("self.offset + 8,"));
        assert!(encoder_section.contains("self.offset + 12,"));
        assert!(encoder_section.contains("self.offset + 20,"));
        assert!(encoder_section.contains("self.offset + 28,"));

        // --- Group encoder wiring ---
        assert!(
            code.contains("BLOCK_LENGTH: u16 = 29"),
            "group encoder BLOCK_LENGTH"
        );
        assert!(
            code.contains("fn orders_count(&mut self, count: u16)"),
            "parent encoder group accessor"
        );
        assert!(
            code.contains("list_orders::OrdersGroupEncoder::wrap(&mut *self.buffer"),
            "parent encoder delegates to module-qualified group encoder"
        );

        // --- Group decoder wiring ---
        assert!(
            code.contains("list_orders::OrdersGroupDecoder"),
            "parent decoder uses module-qualified group decoder"
        );
    }

    #[test]
    fn test_entry_encoder_setter_offsets_correct() {
        let xml = schema_with_group_no_offsets();
        let schema = parse_schema(&xml).expect("Failed to parse schema");
        let ir = SchemaIr::from_schema(&schema);
        let msg_gen = MessageGenerator::new(&ir);
        let code = msg_gen.generate().expect("codegen failed");

        // Find the EntryEncoder section and verify offsets in setters
        let entry_encoder_start = code
            .find("impl<'a> OrdersEntryEncoder<'a>")
            .expect("EntryEncoder impl not found");
        let entry_code = &code[entry_encoder_start..];

        // set_order_id at offset 0
        assert!(
            entry_code.contains("self.offset + 0,"),
            "set_order_id should write at offset 0"
        );
        // set_instrument_id at offset 8
        assert!(
            entry_code.contains("self.offset + 8,"),
            "set_instrument_id should write at offset 8"
        );
        // set_quantity at offset 12
        assert!(
            entry_code.contains("self.offset + 12,"),
            "set_quantity should write at offset 12"
        );
    }

    fn schema_with_group_zero_block_length() -> String {
        r#"<?xml version="1.0" encoding="UTF-8"?>
<sbe:messageSchema xmlns:sbe="http://fixprotocol.io/2016/sbe"
                   package="test" id="1" version="1" byteOrder="littleEndian">
    <types>
        <type name="uint64" primitiveType="uint64"/>
        <type name="uint32" primitiveType="uint32"/>
    </types>
    <sbe:message name="ListOrders" id="19" blockLength="0">
        <group name="orders" id="100" dimensionType="groupSizeEncoding" blockLength="0">
            <field name="orderId" id="1" type="uint64" offset="0"/>
            <field name="instrumentId" id="2" type="uint32"/>
            <field name="quantity" id="3" type="uint64"/>
        </group>
    </sbe:message>
</sbe:messageSchema>"#
            .to_string()
    }

    #[test]
    fn test_group_encoder_block_length_from_xml() {
        let xml = schema_with_group_no_offsets();
        let schema = parse_schema(&xml).expect("Failed to parse schema");
        let ir = SchemaIr::from_schema(&schema);
        let msg_gen = MessageGenerator::new(&ir);
        let code = msg_gen.generate().expect("codegen failed");

        assert!(
            code.contains("BLOCK_LENGTH: u16 = 20"),
            "BLOCK_LENGTH should use the explicit XML blockLength=20"
        );
    }

    #[test]
    fn test_group_encoder_block_length_computed() {
        let xml = schema_with_group_zero_block_length();
        let schema = parse_schema(&xml).expect("Failed to parse schema");
        let ir = SchemaIr::from_schema(&schema);
        let msg_gen = MessageGenerator::new(&ir);
        let code = msg_gen.generate().expect("codegen failed");

        // uint64(8) + uint32(4) + uint64(8) = 20 bytes total
        assert!(
            code.contains("BLOCK_LENGTH: u16 = 20"),
            "BLOCK_LENGTH should be auto-computed as 20 when XML blockLength=0"
        );
    }

    #[test]
    fn test_entry_encoder_wrap_is_pub() {
        let xml = schema_with_group_no_offsets();
        let schema = parse_schema(&xml).expect("Failed to parse schema");
        let ir = SchemaIr::from_schema(&schema);
        let msg_gen = MessageGenerator::new(&ir);
        let code = msg_gen.generate().expect("codegen failed");

        let entry_pos = code
            .find("impl<'a> OrdersEntryEncoder<'a>")
            .expect("EntryEncoder impl not found");
        let entry_section = &code[entry_pos..];

        assert!(
            entry_section.contains("pub fn wrap("),
            "EntryEncoder::wrap should be pub"
        );
    }

    #[test]
    fn test_roundtrip_multi_entry_codegen() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<sbe:messageSchema xmlns:sbe="http://fixprotocol.io/2016/sbe"
                   package="test" id="1" version="1" byteOrder="littleEndian">
    <types>
        <type name="uint64" primitiveType="uint64"/>
        <type name="uint32" primitiveType="uint32"/>
    </types>
    <sbe:message name="ListOrders" id="19" blockLength="8">
        <field name="requestId" id="1" type="uint64" offset="0"/>
        <group name="orders" id="100" dimensionType="groupSizeEncoding" blockLength="0">
            <field name="orderId" id="10" type="uint64" offset="0"/>
            <field name="instrumentId" id="11" type="uint32"/>
            <field name="quantity" id="12" type="uint64"/>
        </group>
    </sbe:message>
</sbe:messageSchema>"#;

        let schema = parse_schema(xml).expect("Failed to parse schema");
        let ir = SchemaIr::from_schema(&schema);
        let msg_gen = MessageGenerator::new(&ir);
        let code = msg_gen.generate().expect("codegen failed");

        // BLOCK_LENGTH should be auto-computed: uint64(8) + uint32(4) + uint64(8) = 20
        assert!(
            code.contains("BLOCK_LENGTH: u16 = 20"),
            "group encoder BLOCK_LENGTH should be 20, not 0"
        );

        // next_entry advances by BLOCK_LENGTH (not 0)
        assert!(
            code.contains("self.offset += Self::BLOCK_LENGTH as usize"),
            "next_entry should advance offset by BLOCK_LENGTH"
        );

        // encoded_length uses BLOCK_LENGTH * count
        assert!(
            code.contains(
                "GroupHeader::ENCODED_LENGTH + Self::BLOCK_LENGTH as usize * self.count as usize"
            ),
            "encoded_length should use BLOCK_LENGTH * count"
        );

        // GroupHeader written with BLOCK_LENGTH
        assert!(
            code.contains("GroupHeader::new(Self::BLOCK_LENGTH, count)"),
            "group header should be written with BLOCK_LENGTH"
        );

        // Parent encoder accessor exists
        assert!(
            code.contains("fn orders_count(&mut self, count: u16)"),
            "parent encoder should have group accessor"
        );

        // Entry encoder wrap is public
        let entry_pos = code
            .find("impl<'a> OrdersEntryEncoder<'a>")
            .expect("EntryEncoder impl not found");
        let entry_section = &code[entry_pos..];
        assert!(
            entry_section.contains("pub fn wrap("),
            "EntryEncoder::wrap should be pub for external consumers"
        );
    }

    // ---------------------------------------------------------------------
    // Var data (<data>) generation, issue #59
    // ---------------------------------------------------------------------

    const VAR_DATA_TYPES: &str = r#"
        <type name="uint8" primitiveType="uint8"/>
        <type name="uint16" primitiveType="uint16"/>
        <type name="uint32" primitiveType="uint32"/>
        <type name="uint64" primitiveType="uint64"/>
        <type name="int64" primitiveType="int64"/>
        <composite name="varStringEncoding">
            <type name="length" primitiveType="uint16"/>
            <type name="varData" primitiveType="uint8" length="0" characterEncoding="UTF-8"/>
        </composite>
        <composite name="varDataEncoding8">
            <type name="length" primitiveType="uint8"/>
            <type name="varData" primitiveType="uint8" length="0"/>
        </composite>
        <composite name="varDataEncoding32">
            <type name="length" primitiveType="uint32"/>
            <type name="varData" primitiveType="uint8" length="0"/>
        </composite>
        <composite name="varDataEncoding64">
            <type name="length" primitiveType="int64"/>
            <type name="varData" primitiveType="uint8" length="0"/>
        </composite>"#;

    fn schema_with(messages: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<sbe:messageSchema xmlns:sbe="http://fixprotocol.io/2016/sbe"
                   package="test" id="1" version="1" byteOrder="littleEndian">
    <types>{VAR_DATA_TYPES}</types>
    {messages}
</sbe:messageSchema>"#
        )
    }

    fn generate(messages: &str) -> Result<String, CodegenError> {
        let xml = schema_with(messages);
        let schema = parse_schema(&xml).expect("Failed to parse schema");
        let ir = SchemaIr::from_schema(&schema);
        MessageGenerator::new(&ir).generate()
    }

    fn generate_ok(messages: &str) -> String {
        generate(messages).expect("codegen failed")
    }

    /// One fixed field, one flat group, then a uint16 and a uint8 var data field.
    const MSG_GROUP_AND_VAR_DATA: &str = r#"
    <sbe:message name="Quote" id="1" blockLength="8">
        <field name="qty" id="1" type="uint64" offset="0"/>
        <group name="legs" id="10" dimensionType="groupSizeEncoding" blockLength="8">
            <field name="legId" id="11" type="uint64" offset="0"/>
        </group>
        <data name="label" id="2" type="varStringEncoding"/>
        <data name="payload" id="3" type="varDataEncoding8"/>
    </sbe:message>"#;

    /// No groups, one uint32-length var data field.
    const MSG_ONLY_VAR_DATA: &str = r#"
    <sbe:message name="Blob" id="2" blockLength="0">
        <data name="rawData" id="1" type="varDataEncoding32"/>
    </sbe:message>"#;

    /// Two flat groups, no var data.
    const MSG_TWO_GROUPS: &str = r#"
    <sbe:message name="ListOrders" id="3" blockLength="8">
        <field name="requestId" id="1" type="uint64" offset="0"/>
        <group name="orders" id="10" dimensionType="groupSizeEncoding" blockLength="8">
            <field name="orderId" id="11" type="uint64" offset="0"/>
        </group>
        <group name="fills" id="20" dimensionType="groupSizeEncoding" blockLength="8">
            <field name="fillId" id="21" type="uint64" offset="0"/>
        </group>
    </sbe:message>"#;

    /// Fixed fields only.
    const MSG_FIXED_ONLY: &str = r#"
    <sbe:message name="Ping" id="4" blockLength="8">
        <field name="ts" id="1" type="uint64" offset="0"/>
    </sbe:message>"#;

    fn section<'a>(code: &'a str, start: &str, end: &str) -> &'a str {
        let from = code
            .find(start)
            .unwrap_or_else(|| panic!("missing '{start}'"));
        let rest = &code[from..];
        let to = rest
            .find(end)
            .unwrap_or_else(|| panic!("missing '{end}' after '{start}'"));
        &rest[..to]
    }

    #[test]
    fn test_var_data_decoder_emits_slice_and_str_accessors() {
        let code = generate_ok(MSG_GROUP_AND_VAR_DATA);
        let decoder = section(&code, "impl<'a> QuoteDecoder<'a>", "impl<'a> SbeDecoder");

        assert!(decoder.contains("fn label_offset(&self) -> usize"));
        assert!(decoder.contains("pub fn label(&self) -> &'a [u8]"));
        assert!(decoder.contains("pub fn label_as_str(&self) -> &'a str"));
        assert!(decoder.contains("std::str::from_utf8(self.label()).unwrap_or(\"\")"));
        assert!(decoder.contains("pub fn payload(&self) -> &'a [u8]"));
        assert!(decoder.contains("pub fn payload_as_str(&self) -> &'a str"));
        assert!(decoder.contains("/// Var data field: label (id=2, length header: uint16)."));
    }

    #[test]
    fn test_var_data_encoder_emits_setter_and_limit_cursor() {
        let code = generate_ok(MSG_GROUP_AND_VAR_DATA);
        let encoder = section(&code, "pub struct QuoteEncoder<'a>", "/// Types for Quote");

        assert!(encoder.contains("    limit: usize,\n"));
        assert!(encoder.contains(
            "let limit = offset + MessageHeader::ENCODED_LENGTH + Self::BLOCK_LENGTH as usize;"
        ));
        assert!(encoder.contains(
            "pub const fn encoded_length(&self) -> usize {\n        self.limit - self.offset"
        ));
        assert!(encoder.contains("pub fn set_label(&mut self, value: &[u8]) -> &mut Self"));
        assert!(encoder.contains("let Ok(len) = u16::try_from(value.len()) else {"));
        assert!(encoder.contains("exceeds u16::MAX"));
        assert!(encoder.contains("self.buffer.put_u16_le(self.limit, len);"));
        assert!(encoder.contains("let start = self.limit + 2;"));
        assert!(encoder.contains("self.buffer.put_bytes(start, value);"));
        assert!(encoder.contains("self.limit = start + value.len();"));
        assert!(encoder.contains("pub fn set_payload(&mut self, value: &[u8]) -> &mut Self"));
        assert!(encoder.contains("self.buffer.put_u8(self.limit, len);"));
        assert!(encoder.contains("let start = self.limit + 1;"));
    }

    #[test]
    fn test_var_data_header_width_uint8_uses_get_u8() {
        let code = generate_ok(MSG_GROUP_AND_VAR_DATA);
        let getter = section(&code, "pub fn payload(&self)", "pub fn payload_as_str");

        assert!(getter.contains("let len = self.buffer.get_u8(pos) as usize;"));
        assert!(getter.contains("let start = pos + 1;"));
    }

    #[test]
    fn test_var_data_header_width_uint16_uses_get_u16_le() {
        let code = generate_ok(MSG_GROUP_AND_VAR_DATA);
        let getter = section(&code, "pub fn label(&self)", "pub fn label_as_str");

        assert!(getter.contains("let len = self.buffer.get_u16_le(pos) as usize;"));
        assert!(getter.contains("let start = pos + 2;"));
    }

    #[test]
    fn test_var_data_header_width_uint32_uses_get_u32_le() {
        let code = generate_ok(MSG_ONLY_VAR_DATA);

        assert!(code.contains("pub fn raw_data(&self) -> &'a [u8]"));
        assert!(code.contains("let len = self.buffer.get_u32_le(pos) as usize;"));
        assert!(code.contains("let start = pos + 4;"));
        assert!(code.contains("let Ok(len) = u32::try_from(value.len()) else {"));
        assert!(code.contains("self.buffer.put_u32_le(self.limit, len);"));
    }

    #[test]
    fn test_var_data_offset_chain_follows_last_group() {
        let code = generate_ok(MSG_GROUP_AND_VAR_DATA);

        let label = section(&code, "fn label_offset(&self)", "/// Var data field: label");
        assert!(
            label.contains("self.group_offset(1)"),
            "first var data field must start after the last (1) group: {label}"
        );

        let payload = section(
            &code,
            "fn payload_offset(&self)",
            "/// Var data field: payload",
        );
        assert!(payload.contains("let pos = self.label_offset();"));
        assert!(payload.contains("pos + 2 + self.buffer.get_u16_le(pos) as usize"));
    }

    #[test]
    fn test_var_data_without_groups_starts_after_block() {
        let code = generate_ok(MSG_ONLY_VAR_DATA);
        let offset = section(
            &code,
            "fn raw_data_offset(&self)",
            "/// Var data field: rawData",
        );

        assert!(offset.contains("self.offset + Self::BLOCK_LENGTH as usize"));
        assert!(!code.contains("fn group_offset("), "no groups, no walker");
    }

    #[test]
    fn test_multiple_groups_use_group_offset_walk() {
        let code = generate_ok(MSG_TWO_GROUPS);

        assert!(code.contains("fn group_offset(&self, index: usize) -> usize"));
        assert!(code.contains("pos += GroupHeader::wrap(self.buffer, pos).group_size();"));
        assert!(
            code.contains(
                "list_orders::OrdersGroupDecoder::wrap(self.buffer, self.group_offset(0))"
            )
        );
        assert!(
            code.contains(
                "list_orders::FillsGroupDecoder::wrap(self.buffer, self.group_offset(1))"
            )
        );

        // Encoder places each group at the cursor and advances it.
        assert!(code.contains("let offset = self.limit;"));
        assert!(code.contains(
            "self.limit += GroupHeader::ENCODED_LENGTH + list_orders::OrdersGroupEncoder::BLOCK_LENGTH as usize * count as usize;"
        ));
        assert!(code.contains(
            "self.limit += GroupHeader::ENCODED_LENGTH + list_orders::FillsGroupEncoder::BLOCK_LENGTH as usize * count as usize;"
        ));
        assert!(
            code.contains("list_orders::FillsGroupEncoder::wrap(&mut *self.buffer, offset, count)")
        );
    }

    #[test]
    fn test_encoded_length_includes_groups_and_var_data() {
        let code = generate_ok(MSG_GROUP_AND_VAR_DATA);
        let decoder_impl = section(
            &code,
            "impl<'a> SbeDecoder<'a> for QuoteDecoder",
            "/// Quote Encoder",
        );
        assert!(decoder_impl.contains("let pos = self.payload_offset();"));
        assert!(decoder_impl.contains("let end = pos + 1 + self.buffer.get_u8(pos) as usize;"));
        assert!(decoder_impl.contains("MessageHeader::ENCODED_LENGTH + (end - self.offset)"));
    }

    #[test]
    fn test_encoded_length_groups_only_walks_all_groups() {
        let code = generate_ok(MSG_TWO_GROUPS);
        let decoder_impl = section(
            &code,
            "impl<'a> SbeDecoder<'a> for ListOrdersDecoder",
            "/// ListOrders Encoder",
        );
        assert!(
            decoder_impl
                .contains("MessageHeader::ENCODED_LENGTH + (self.group_offset(2) - self.offset)")
        );
    }

    #[test]
    fn test_encoded_length_fixed_only_stays_constant() {
        let code = generate_ok(MSG_FIXED_ONLY);
        let decoder_impl = section(
            &code,
            "impl<'a> SbeDecoder<'a> for PingDecoder",
            "/// Ping Encoder",
        );
        assert!(
            decoder_impl.contains("MessageHeader::ENCODED_LENGTH + Self::BLOCK_LENGTH as usize")
        );
        assert!(
            !code.contains("_offset(&self)"),
            "no var data helpers expected"
        );
    }

    #[test]
    fn test_var_data_in_group_returns_unsupported_err() {
        let err = generate(
            r#"
    <sbe:message name="Quote" id="1" blockLength="0">
        <group name="legs" id="10" dimensionType="groupSizeEncoding" blockLength="8">
            <field name="legId" id="11" type="uint64" offset="0"/>
            <data name="note" id="12" type="varStringEncoding"/>
        </group>
    </sbe:message>"#,
        )
        .expect_err("var data inside a group must be rejected");

        assert!(matches!(err, CodegenError::Unsupported { .. }), "{err:?}");
        let msg = err.to_string();
        assert!(msg.contains("<data> inside repeating group"), "{msg}");
        assert!(msg.contains("message 'Quote', group 'legs'"), "{msg}");
    }

    #[test]
    fn test_var_data_in_nested_group_returns_unsupported_err() {
        let err = generate(
            r#"
    <sbe:message name="Quote" id="1" blockLength="0">
        <group name="legs" id="10" dimensionType="groupSizeEncoding" blockLength="8">
            <field name="legId" id="11" type="uint64" offset="0"/>
            <group name="fills" id="20" dimensionType="groupSizeEncoding" blockLength="8">
                <field name="fillId" id="21" type="uint64" offset="0"/>
                <data name="note" id="22" type="varStringEncoding"/>
            </group>
        </group>
    </sbe:message>"#,
        )
        .expect_err("var data inside a nested group must be rejected");

        assert!(matches!(err, CodegenError::Unsupported { .. }), "{err:?}");
        assert!(err.to_string().contains("group 'fills'"), "{err}");
    }

    #[test]
    fn test_var_data_after_nested_group_returns_unsupported_err() {
        let err = generate(
            r#"
    <sbe:message name="Quote" id="1" blockLength="0">
        <group name="legs" id="10" dimensionType="groupSizeEncoding" blockLength="8">
            <field name="legId" id="11" type="uint64" offset="0"/>
            <group name="fills" id="20" dimensionType="groupSizeEncoding" blockLength="8">
                <field name="fillId" id="21" type="uint64" offset="0"/>
            </group>
        </group>
        <data name="label" id="2" type="varStringEncoding"/>
    </sbe:message>"#,
        )
        .expect_err("var data after a group with nested groups must be rejected");

        assert!(matches!(err, CodegenError::Unsupported { .. }), "{err:?}");
        let msg = err.to_string();
        assert!(
            msg.contains("<data> after a repeating group with nested groups"),
            "{msg}"
        );
        assert!(msg.contains("group 'legs'"), "{msg}");
    }

    #[test]
    fn test_nested_group_without_var_data_still_generates() {
        let code = generate_ok(
            r#"
    <sbe:message name="Quote" id="1" blockLength="0">
        <group name="legs" id="10" dimensionType="groupSizeEncoding" blockLength="8">
            <field name="legId" id="11" type="uint64" offset="0"/>
            <group name="fills" id="20" dimensionType="groupSizeEncoding" blockLength="8">
                <field name="fillId" id="21" type="uint64" offset="0"/>
            </group>
        </group>
    </sbe:message>"#,
        );
        assert!(code.contains("pub struct QuoteDecoder"));
    }

    #[test]
    fn test_var_data_unknown_type_returns_unknown_type_err() {
        let err = generate(
            r#"
    <sbe:message name="Quote" id="1" blockLength="0">
        <data name="label" id="2" type="noSuchEncoding"/>
    </sbe:message>"#,
        )
        .expect_err("unknown var data type must be rejected");

        assert!(matches!(err, CodegenError::UnknownType { .. }), "{err:?}");
        let msg = err.to_string();
        assert!(msg.contains("noSuchEncoding"), "{msg}");
        assert!(msg.contains("Quote.label"), "{msg}");
    }

    #[test]
    fn test_var_data_non_composite_type_returns_unsupported_err() {
        let err = generate(
            r#"
    <sbe:message name="Quote" id="1" blockLength="0">
        <data name="label" id="2" type="uint64"/>
    </sbe:message>"#,
        )
        .expect_err("primitive var data type must be rejected");

        assert!(matches!(err, CodegenError::Unsupported { .. }), "{err:?}");
        assert!(err.to_string().contains("is not a composite"), "{err}");
    }

    #[test]
    fn test_var_data_length_int64_returns_unsupported_err() {
        let err = generate(
            r#"
    <sbe:message name="Quote" id="1" blockLength="0">
        <data name="label" id="2" type="varDataEncoding64"/>
    </sbe:message>"#,
        )
        .expect_err("int64 length header must be rejected");

        assert!(matches!(err, CodegenError::Unsupported { .. }), "{err:?}");
        let msg = err.to_string();
        assert!(msg.contains("var data length encoding 'int64'"), "{msg}");
        assert!(msg.contains("message 'Quote', data 'label'"), "{msg}");
    }
}
