//! Message encoder/decoder code generation.

use ironsbe_schema::ir::{
    ResolvedField, ResolvedGroup, ResolvedMessage, SchemaIr, TypeKind, to_snake_case,
};
use ironsbe_schema::types::PrimitiveType;

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
    #[must_use]
    pub fn generate(&self) -> String {
        let mut output = String::new();

        for msg in &self.ir.messages {
            output.push_str(&self.generate_decoder(msg));
            output.push_str(&self.generate_encoder(msg));

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

        output
    }

    /// Generates a message decoder.
    fn generate_decoder(&self, msg: &ResolvedMessage) -> String {
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

        // Group accessors
        let mut group_offset = msg.block_length as usize;
        for group in &msg.groups {
            output.push_str(&self.generate_group_accessor(group, group_offset, &msg.name));
            group_offset += 4; // Group header size
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

        output.push_str("    fn encoded_length(&self) -> usize {\n");
        output.push_str("        MessageHeader::ENCODED_LENGTH + Self::BLOCK_LENGTH as usize\n");
        output.push_str("    }\n");
        output.push_str("}\n\n");

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
    fn generate_group_accessor(
        &self,
        group: &ResolvedGroup,
        offset: usize,
        msg_name: &str,
    ) -> String {
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
            "        {}::wrap(self.buffer, self.offset + {})\n",
            qualified, offset
        ));
        output.push_str("    }\n\n");

        output
    }

    /// Generates a message encoder.
    fn generate_encoder(&self, msg: &ResolvedMessage) -> String {
        let mut output = String::new();
        let encoder_name = msg.encoder_name();

        // Struct definition
        output.push_str(&format!("/// {} Encoder.\n", msg.name));
        output.push_str(&format!("pub struct {}<'a> {{\n", encoder_name));
        output.push_str("    buffer: &'a mut [u8],\n");
        output.push_str("    offset: usize,\n");
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
        output.push_str("    #[inline]\n");
        output.push_str("    pub fn wrap(buffer: &'a mut [u8], offset: usize) -> Self {\n");
        output.push_str("        let mut encoder = Self { buffer, offset };\n");
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
        output.push_str("    /// Returns the encoded length of the message.\n");
        output.push_str("    #[must_use]\n");
        output.push_str("    pub const fn encoded_length(&self) -> usize {\n");
        output.push_str("        MessageHeader::ENCODED_LENGTH + Self::BLOCK_LENGTH as usize\n");
        output.push_str("    }\n\n");

        // Field setters
        for field in &msg.fields {
            output.push_str(&self.generate_field_setter(field));
        }

        // Group encoder accessors
        let mut group_offset = msg.block_length as usize;
        for group in &msg.groups {
            output.push_str(&self.generate_group_encoder_accessor(group, group_offset, &msg.name));
            group_offset += 4; // Group header size
        }

        output.push_str("}\n\n");

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
    fn generate_group_encoder_accessor(
        &self,
        group: &ResolvedGroup,
        offset: usize,
        msg_name: &str,
    ) -> String {
        let mut output = String::new();
        let qualified = format!("{}::{}", to_snake_case(msg_name), group.encoder_name());

        output.push_str(&format!(
            "    /// Begin encoding the {} repeating group.\n",
            group.name
        ));
        output.push_str(&format!(
            "    pub fn {}_count(&mut self, count: u16) -> {}<'_> {{\n",
            to_snake_case(&group.name),
            qualified
        ));
        output.push_str(&format!(
            "        {}::wrap(&mut *self.buffer, self.offset + MessageHeader::ENCODED_LENGTH + {}, count)\n",
            qualified, offset
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
        let code = msg_gen.generate();

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
        let code = msg_gen.generate();

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
        let code = msg_gen.generate();

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
        let code = msg_gen.generate();

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
        let code = msg_gen.generate();

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
        let code = msg_gen.generate();

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
        let code = msg_gen.generate();

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
        let code = msg_gen.generate();

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
        let code = msg_gen.generate();

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
        let code = msg_gen.generate();

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
        let code = msg_gen.generate();

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
        let code = msg_gen.generate();

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
        let code = msg_gen.generate();

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
        let code = msg_gen.generate();

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
}
