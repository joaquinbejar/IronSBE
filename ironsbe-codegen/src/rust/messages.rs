//! Message encoder/decoder code generation.

use ironsbe_schema::ir::{ResolvedField, ResolvedGroup, ResolvedMessage, SchemaIr, to_snake_case};
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

            // Generate group decoders/encoders
            for group in &msg.groups {
                output.push_str(&self.generate_group_decoder(group));
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
            output.push_str(&self.generate_group_accessor(group, group_offset));
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
            // Scalar field
            let rust_type = &field.rust_type;
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

        output
    }

    /// Generates a group accessor method.
    fn generate_group_accessor(&self, group: &ResolvedGroup, offset: usize) -> String {
        let mut output = String::new();
        let group_decoder = group.decoder_name();

        output.push_str(&format!("    /// Access {} repeating group.\n", group.name));
        output.push_str("    #[inline]\n");
        output.push_str("    #[must_use]\n");
        output.push_str(&format!(
            "    pub fn {}(&self) -> {}<'a> {{\n",
            to_snake_case(&group.name),
            group_decoder
        ));
        output.push_str(&format!(
            "        {}::wrap(self.buffer, self.offset + {})\n",
            group_decoder, offset
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
            // Scalar field
            let rust_type = &field.rust_type;
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
