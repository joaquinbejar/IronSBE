//! Message encoder/decoder code generation.
//!
//! Emits, per message, a zero-copy decoder and a cursor-based encoder, and a
//! message-scoped module holding the codecs of its repeating groups. Field,
//! var data and group emission live in sibling modules.

use ironsbe_schema::ir::{ResolvedMessage, SchemaIr, to_snake_case};

use crate::error::CodegenError;
use crate::rust::fields::{generate_field_getter, generate_field_setter};
use crate::rust::groups::{
    GroupLayout, generate_group_accessor, generate_group_decoder, generate_group_encoder,
    generate_group_encoder_accessor, generate_group_offset_walker,
};
use crate::rust::names::MESSAGE_RESERVED;
use crate::rust::readers::{generate_group_reader, generate_message_reader, message_needs_reader};
use crate::rust::var_data::{
    VarDataInfo, end_offset_parts, generate_var_data_getter, generate_var_data_setter,
    resolve_var_data,
};
use crate::rust::var_parts::{collect_var_parts, generate_encoder_parts_guard};

/// Byte offset just past the root block, as seen from a message decoder.
const MESSAGE_BLOCK_END: &str = "self.offset + Self::BLOCK_LENGTH as usize";

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
    /// generator cannot emit correct code for (currently var data length
    /// headers other than `uint8` / `uint16` / `uint32`), and
    /// [`CodegenError::UnknownType`] for `<data>` elements whose type is not
    /// declared in the schema.
    pub fn generate(&self) -> Result<String, CodegenError> {
        let mut output = String::new();

        for msg in &self.ir.messages {
            let context = format!("message '{}'", msg.name);
            let var_data = resolve_var_data(
                self.ir,
                &context,
                &msg.name,
                &msg.var_data,
                MESSAGE_RESERVED,
            )?;
            let groups = msg
                .groups
                .iter()
                .map(|g| GroupLayout::resolve(self.ir, &context, &msg.name, g))
                .collect::<Result<Vec<_>, _>>()?;

            output.push_str(&self.generate_decoder(msg, &groups, &var_data));
            output.push_str(&self.generate_encoder(msg, &groups, &var_data));

            // Sequential reader, only where random access has to walk the wire
            let needs_reader = message_needs_reader(&groups, &var_data);
            if needs_reader {
                output.push_str(&generate_message_reader(self.ir, msg, &groups, &var_data));
            }

            // Generate group decoders, encoders and readers in a message-scoped module
            if !groups.is_empty() {
                let mod_name = to_snake_case(&msg.name);
                output.push_str(&format!("/// Types for {} repeating groups.\n", msg.name));
                output.push_str(&format!("pub mod {} {{\n", mod_name));
                output.push_str("    use super::*;\n\n");
                for layout in &groups {
                    output.push_str(&generate_group_decoder(self.ir, layout));
                    output.push_str(&generate_group_encoder(self.ir, layout));
                    if needs_reader {
                        output.push_str(&generate_group_reader(self.ir, layout));
                    }
                }
                output.push_str("}\n\n");
            }
        }

        Ok(output)
    }

    /// Generates a message decoder.
    fn generate_decoder(
        &self,
        msg: &ResolvedMessage,
        groups: &[GroupLayout<'_>],
        var_data: &[VarDataInfo],
    ) -> String {
        let mut output = String::new();
        let decoder_name = msg.decoder_name();
        let mod_name = to_snake_case(&msg.name);

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
            output.push_str(&generate_field_getter(self.ir, field, MESSAGE_RESERVED));
        }

        // Group accessors. Groups follow the fixed block back to back, so the
        // offset of group `i` depends on the extent of groups `0..i` and has
        // to be walked on the wire.
        let qualified: Vec<String> = groups
            .iter()
            .map(|g| format!("{mod_name}::{}", g.group.decoder_name()))
            .collect();
        if !groups.is_empty() {
            output.push_str(&generate_group_offset_walker(MESSAGE_BLOCK_END, &qualified));
        }
        for (index, (layout, decoder_type)) in groups.iter().zip(&qualified).enumerate() {
            output.push_str(&generate_group_accessor(
                &layout.group.name,
                decoder_type,
                index,
                MESSAGE_RESERVED,
            ));
        }

        // Var data accessors (after all groups, in schema order)
        for index in 0..var_data.len() {
            output.push_str(&generate_var_data_getter(
                index,
                var_data,
                groups.len(),
                MESSAGE_BLOCK_END,
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
            groups.len(),
        ));
        output.push_str("}\n\n");

        output
    }

    /// Generates `SbeDecoder::encoded_length` for a message decoder.
    ///
    /// Covers header + fixed block + every repeating group + every var data
    /// field, reading the variable parts from the wire.
    fn generate_decoder_encoded_length(var_data: &[VarDataInfo], group_count: usize) -> String {
        let mut output = String::new();

        output.push_str("    fn encoded_length(&self) -> usize {\n");
        if var_data.is_empty() && group_count == 0 {
            output
                .push_str("        MessageHeader::ENCODED_LENGTH + Self::BLOCK_LENGTH as usize\n");
        } else {
            let (prelude, end_expr) = end_offset_parts(var_data, group_count, MESSAGE_BLOCK_END);
            output.push_str(&prelude);
            output.push_str(&format!(
                "        MessageHeader::ENCODED_LENGTH + ({end_expr} - self.offset)\n"
            ));
        }
        output.push_str("    }\n");

        output
    }

    /// Generates a message encoder.
    fn generate_encoder(
        &self,
        msg: &ResolvedMessage,
        groups: &[GroupLayout<'_>],
        var_data: &[VarDataInfo],
    ) -> String {
        let mut output = String::new();
        let encoder_name = msg.encoder_name();
        let mod_name = to_snake_case(&msg.name);
        let parts = collect_var_parts(groups, var_data, &format!("{mod_name}::"));
        let has_parts = !parts.is_empty();

        // Struct definition
        output.push_str(&format!("/// {} Encoder.\n", msg.name));
        output.push_str("///\n");
        output.push_str("/// Fixed fields are written at their schema offsets. Repeating groups\n");
        output.push_str("/// and var data fields are appended at a write cursor (`limit`) and\n");
        output.push_str("/// must be written in schema order.");
        if has_parts {
            output.push_str(" Call `finish()` once done: it encodes\n");
            output.push_str("/// every group or var data field not written as empty and returns\n");
            output.push_str("/// the frame length.\n");
        } else {
            output.push('\n');
        }
        output.push_str(&format!("pub struct {}<'a> {{\n", encoder_name));
        output.push_str("    buffer: &'a mut [u8],\n");
        output.push_str("    offset: usize,\n");
        output.push_str("    limit: usize,\n");
        if has_parts {
            output.push_str("    written: u16,\n");
        }
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
        if has_parts {
            output.push_str(
                "        let mut encoder = Self { buffer, offset, limit, written: 0 };\n",
            );
        } else {
            output.push_str("        let mut encoder = Self { buffer, offset, limit };\n");
        }
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
        output.push_str("    /// written through this encoder.");
        if has_parts {
            output.push_str(" Parts not written yet are not\n");
            output.push_str("    /// counted; use `finish()` to complete the message.\n");
        } else {
            output.push('\n');
        }
        output.push_str("    #[inline]\n");
        output.push_str("    #[must_use]\n");
        output.push_str("    pub const fn encoded_length(&self) -> usize {\n");
        output.push_str("        self.limit - self.offset\n");
        output.push_str("    }\n\n");

        // Finish
        output.push_str(&Self::generate_encoder_finish(has_parts));

        // Field setters
        for field in &msg.fields {
            output.push_str(&generate_field_setter(self.ir, field));
        }

        // Group encoder accessors (lend the write cursor to the group encoder)
        for (index, layout) in groups.iter().enumerate() {
            output.push_str(&generate_group_encoder_accessor(
                &layout.group.name,
                &format!("{mod_name}::{}", layout.group.encoder_name()),
                "&mut self.limit",
                index,
            ));
        }

        // Var data setters (append at the write cursor, after all groups)
        for (index, info) in var_data.iter().enumerate() {
            output.push_str(&generate_var_data_setter(
                info,
                "self.limit",
                groups.len() + index,
            ));
        }

        // Variable-part guard (only encoders with parts carry `written`)
        output.push_str(&generate_encoder_parts_guard(&parts, "self.limit"));

        output.push_str("}\n\n");

        output
    }

    /// Generates `finish()` on a message encoder.
    ///
    /// Emitted on every encoder so callers can rely on it: with variable
    /// parts it fills the missing ones and returns the frame length; without
    /// them it is `encoded_length()`.
    fn generate_encoder_finish(has_parts: bool) -> String {
        let mut output = String::new();

        output.push_str("    /// Completes the message and returns its encoded length in bytes:\n");
        output.push_str("    /// header, fixed block and variable section.\n");
        if has_parts {
            output.push_str("    ///\n");
            output.push_str(
                "    /// Every repeating group not begun and every var data field not set is\n",
            );
            output.push_str(
                "    /// encoded as empty (a group header with zero entries, a zero-length\n",
            );
            output.push_str("    /// var data header), so the frame is always well-formed.\n");
            output.push_str("    ///\n");
            output.push_str("    /// # Panics\n");
            output.push_str("    /// Panics if the buffer is too short for the empty headers.\n");
        }
        output.push_str("    #[must_use]\n");
        if has_parts {
            output.push_str("    pub fn finish(mut self) -> usize {\n");
            output.push_str("        self.sbe_fill_to(Self::SBE_VAR_PARTS);\n");
            output.push_str("        self.limit - self.offset\n");
        } else {
            output.push_str("    pub const fn finish(self) -> usize {\n");
            output.push_str("        self.encoded_length()\n");
        }
        output.push_str("    }\n\n");

        output
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

        // next_entry advances the shared cursor by BLOCK_LENGTH (not 0)
        assert!(
            code.contains("*self.limit = offset + Self::BLOCK_LENGTH as usize;"),
            "next_entry should advance the cursor by BLOCK_LENGTH"
        );

        // encoded_length measures what was written through the cursor
        assert!(
            code.contains(
                "pub fn encoded_length(&self) -> usize {\n        *self.limit - self.start"
            ),
            "group encoded_length should be cursor - start"
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

        assert!(decoder.contains("fn sbe_label_offset(&self) -> usize"));
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

        let label = section(
            &code,
            "fn sbe_label_offset(&self)",
            "/// Var data field: label",
        );
        assert!(
            label.contains("self.sbe_group_offset(1)"),
            "first var data field must start after the last (1) group: {label}"
        );

        let payload = section(
            &code,
            "fn sbe_payload_offset(&self)",
            "/// Var data field: payload",
        );
        assert!(payload.contains("let pos = self.sbe_label_offset();"));
        assert!(payload.contains("pos + 2 + self.buffer.get_u16_le(pos) as usize"));
    }

    #[test]
    fn test_var_data_without_groups_starts_after_block() {
        let code = generate_ok(MSG_ONLY_VAR_DATA);
        let offset = section(
            &code,
            "fn sbe_raw_data_offset(&self)",
            "/// Var data field: rawData",
        );

        assert!(offset.contains("self.offset + Self::BLOCK_LENGTH as usize"));
        assert!(
            !code.contains("fn sbe_group_offset("),
            "no groups, no walker"
        );
    }

    #[test]
    fn test_multiple_groups_use_group_offset_walk() {
        let code = generate_ok(MSG_TWO_GROUPS);

        assert!(code.contains("fn sbe_group_offset(&self, index: usize) -> usize"));
        assert!(
            !code.contains("group_size()"),
            "walker must go through per-group end_offset, not GroupHeader::group_size"
        );
        assert!(code.contains(
            "pos = list_orders::OrdersGroupDecoder::wrap(self.buffer, pos).end_offset();"
        ));
        assert!(code.contains(
            "pos = list_orders::FillsGroupDecoder::wrap(self.buffer, pos).end_offset();"
        ));
        assert!(code.contains(
            "list_orders::OrdersGroupDecoder::wrap(self.buffer, self.sbe_group_offset(0))"
        ));
        assert!(code.contains(
            "list_orders::FillsGroupDecoder::wrap(self.buffer, self.sbe_group_offset(1))"
        ));

        // Encoder lends its cursor to each group encoder instead of pre-advancing it.
        let orders_count = section(&code, "pub fn orders_count(", "    }\n");
        assert!(
            !orders_count.contains("self.limit +="),
            "message encoder must not precompute the group extent"
        );
        assert!(code.contains(
            "list_orders::OrdersGroupEncoder::wrap(&mut *self.buffer, &mut self.limit, count)"
        ));
        assert!(code.contains(
            "list_orders::FillsGroupEncoder::wrap(&mut *self.buffer, &mut self.limit, count)"
        ));
    }

    #[test]
    fn test_encoder_with_parts_carries_written_counter_and_guard() {
        let code = generate_ok(MSG_GROUP_AND_VAR_DATA);
        let encoder = section(&code, "pub struct QuoteEncoder<'a> {", "\n}\n");
        assert!(encoder.contains("    written: u16,"));
        assert!(code.contains("let mut encoder = Self { buffer, offset, limit, written: 0 };"));

        let imp = section(&code, "impl<'a> QuoteEncoder<'a> {", "\n}\n\n");
        assert!(imp.contains("const SBE_VAR_PARTS: u16 = 3;"));
        assert!(imp.contains("fn sbe_advance_to(&mut self, target: u16, what: &'static str)"));
        // parts in schema order: the group, then the two var data fields
        assert!(imp.contains("self.sbe_advance_to(0, \"repeating group 'legs'\");"));
        assert!(imp.contains("self.sbe_advance_to(1, \"var data field 'label'\");"));
        assert!(imp.contains("self.sbe_advance_to(2, \"var data field 'payload'\");"));
        // empty headers use the group encoder's block length and the field widths
        assert!(imp.contains(
            "GroupHeader::new(quote::LegsGroupEncoder::BLOCK_LENGTH, 0).encode(self.buffer, self.limit);"
        ));
        assert!(
            imp.contains(
                "self.buffer.put_u16_le(self.limit, 0);\n                self.limit += 2;"
            )
        );
        assert!(
            imp.contains("self.buffer.put_u8(self.limit, 0);\n                self.limit += 1;")
        );
        // finish fills the tail and returns the frame length
        assert!(imp.contains(
            "pub fn finish(mut self) -> usize {\n        self.sbe_fill_to(Self::SBE_VAR_PARTS);\n        self.limit - self.offset\n    }"
        ));
    }

    #[test]
    fn test_encoder_with_flat_groups_only_still_guards_groups() {
        let code = generate_ok(MSG_TWO_GROUPS);
        let imp = section(&code, "impl<'a> ListOrdersEncoder<'a> {", "\n}\n\n");
        assert!(imp.contains("const SBE_VAR_PARTS: u16 = 2;"));
        assert!(imp.contains("self.sbe_advance_to(0, \"repeating group 'orders'\");"));
        assert!(imp.contains("self.sbe_advance_to(1, \"repeating group 'fills'\");"));
        assert!(imp.contains("pub fn finish(mut self) -> usize {"));
    }

    #[test]
    fn test_flat_encoder_has_no_counter_and_const_finish() {
        let code = generate_ok(MSG_FIXED_ONLY);
        let encoder = section(&code, "pub struct PingEncoder<'a> {", "\n}\n");
        assert!(!encoder.contains("written"));
        assert!(code.contains("let mut encoder = Self { buffer, offset, limit };"));

        let imp = section(&code, "impl<'a> PingEncoder<'a> {", "\n}\n\n");
        assert!(!imp.contains("SBE_VAR_PARTS"));
        assert!(!imp.contains("sbe_advance_to"));
        assert!(imp.contains(
            "pub const fn finish(self) -> usize {\n        self.encoded_length()\n    }"
        ));
    }

    #[test]
    fn test_variable_entry_encoder_guards_and_fills_on_drop() {
        let code = generate_ok(MSG_NESTED_WITH_VAR_DATA);

        // orders entries: nested group `fills` is part 0, `memo` is part 1
        let orders = section(&code, "pub struct OrdersEntryEncoder<'a> {", "\n}\n");
        assert!(orders.contains("    limit: &'a mut usize,\n    written: u16,"));
        let orders_impl = section(&code, "impl<'a> OrdersEntryEncoder<'a> {", "\n}\n\n");
        assert!(orders_impl.contains("Self { buffer, offset, limit, written: 0 }"));
        assert!(orders_impl.contains("const SBE_VAR_PARTS: u16 = 2;"));
        assert!(orders_impl.contains("self.sbe_advance_to(0, \"repeating group 'fills'\");"));
        assert!(orders_impl.contains("self.sbe_advance_to(1, \"var data field 'memo'\");"));
        assert!(orders_impl.contains(
            "GroupHeader::new(FillsGroupEncoder::BLOCK_LENGTH, 0).encode(self.buffer, *self.limit);"
        ));
        assert!(orders_impl.contains("*self.limit += GroupHeader::ENCODED_LENGTH;"));
        assert!(orders_impl.contains(
            "self.buffer.put_u16_le(*self.limit, 0);\n                *self.limit += 2;"
        ));
        assert!(code.contains("impl Drop for OrdersEntryEncoder<'_> {"));
        assert!(code.contains("impl Drop for FillsEntryEncoder<'_> {"));

        // flat entries stay untouched
        let flags = section(&code, "pub struct FlagsEntryEncoder<'a> {", "\n}\n");
        assert!(!flags.contains("written"));
        assert!(!code.contains("impl Drop for FlagsEntryEncoder"));
        let flags_impl = section(&code, "impl<'a> FlagsEntryEncoder<'a> {", "\n}\n\n");
        assert!(!flags_impl.contains("SBE_VAR_PARTS"));
    }

    #[test]
    fn test_flat_group_encoder_borrows_cursor_and_keeps_entry_api() {
        let code = generate_ok(MSG_TWO_GROUPS);
        let group = section(
            &code,
            "pub struct OrdersGroupEncoder<'a>",
            "/// orders Entry Encoder",
        );

        assert!(group.contains("    limit: &'a mut usize,\n    start: usize,\n"));
        assert!(group.contains(
            "pub fn wrap(buffer: &'a mut [u8], limit: &'a mut usize, count: u16) -> Self"
        ));
        assert!(group.contains("let start = *limit;"));
        assert!(group.contains("*limit = start + GroupHeader::ENCODED_LENGTH;"));
        assert!(group.contains("GroupHeader::new(Self::BLOCK_LENGTH, count)"));
        assert!(group.contains("pub const fn count(&self) -> u16"));
        assert!(group.contains("Some(OrdersEntryEncoder::wrap(&mut *self.buffer, offset))"));

        let entry = section(
            &code,
            "pub struct OrdersEntryEncoder<'a>",
            "/// fills Group Encoder",
        );
        assert!(
            !entry.contains("limit"),
            "flat entry encoder keeps the 0.5 shape: {entry}"
        );
        assert!(entry.contains("pub fn wrap(buffer: &'a mut [u8], offset: usize) -> Self"));
    }

    #[test]
    fn test_encoded_length_includes_groups_and_var_data() {
        let code = generate_ok(MSG_GROUP_AND_VAR_DATA);
        let decoder_impl = section(
            &code,
            "impl<'a> SbeDecoder<'a> for QuoteDecoder",
            "/// Quote Encoder",
        );
        assert!(decoder_impl.contains("let pos = self.sbe_payload_offset();"));
        assert!(decoder_impl.contains(
            "MessageHeader::ENCODED_LENGTH + (pos + 1 + self.buffer.get_u8(pos) as usize - self.offset)"
        ));
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
            decoder_impl.contains(
                "MessageHeader::ENCODED_LENGTH + (self.sbe_group_offset(2) - self.offset)"
            )
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

    // ---------------------------------------------------------------------
    // <data> and nested groups inside repeating groups, issue #61
    // ---------------------------------------------------------------------

    /// Issue #61 shape: a group whose entries carry a fixed field and a
    /// uint16-length var data field, plus message-level var data after it.
    const MSG_VAR_DATA_IN_GROUP: &str = r#"
    <sbe:message name="Quote" id="1" blockLength="4">
        <field name="requestId" id="1" type="uint32" offset="0"/>
        <group name="legs" id="10" dimensionType="groupSizeEncoding" blockLength="4">
            <field name="legQty" id="11" type="uint32" offset="0"/>
            <data name="legTag" id="12" type="varStringEncoding"/>
            <data name="legNote" id="13" type="varDataEncoding8"/>
        </group>
        <data name="comment" id="2" type="varStringEncoding"/>
    </sbe:message>"#;

    /// Nested group whose inner entries carry var data, followed by var data
    /// on the outer entry, a second flat group and message-level var data.
    const MSG_NESTED_WITH_VAR_DATA: &str = r#"
    <sbe:message name="Nested" id="5" blockLength="0">
        <group name="orders" id="10" dimensionType="groupSizeEncoding" blockLength="8">
            <field name="orderId" id="11" type="uint64" offset="0"/>
            <group name="fills" id="20" dimensionType="groupSizeEncoding" blockLength="8">
                <field name="fillId" id="21" type="uint64" offset="0"/>
                <data name="note" id="22" type="varDataEncoding8"/>
            </group>
            <data name="memo" id="12" type="varStringEncoding"/>
        </group>
        <group name="flags" id="30" dimensionType="groupSizeEncoding" blockLength="1">
            <field name="flag" id="31" type="uint8" offset="0"/>
        </group>
        <data name="trailer" id="2" type="varDataEncoding32"/>
    </sbe:message>"#;

    #[test]
    fn test_flat_group_keeps_fixed_stride_and_entry_api() {
        let code = generate_ok(MSG_TWO_GROUPS);
        let group = section(
            &code,
            "impl<'a> OrdersGroupDecoder<'a>",
            "/// orders Entry Decoder",
        );
        assert!(
            group.contains(
                "pub fn end_offset(self) -> usize {\n        self.offset + self.block_length as usize * (self.count - self.index) as usize"
            ),
            "fixed-stride group must compute its end in O(1): {group}"
        );
        assert!(group.contains("self.offset += self.block_length as usize;"));
        assert!(!group.contains("entry.end_offset()"));

        let entry = section(
            &code,
            "impl<'a> OrdersEntryDecoder<'a>",
            "/// fills Group Decoder",
        );
        assert!(entry.contains("fn wrap(buffer: &'a [u8], offset: usize, block_length: u16)"));
        assert!(entry.contains(
            "pub fn end_offset(&self) -> usize {\n        self.offset + self.block_length as usize\n"
        ));
        assert!(
            !entry.contains("fn sbe_group_offset("),
            "flat entry has no nested walker"
        );
    }

    #[test]
    fn test_var_data_in_group_no_longer_returns_unsupported() {
        let result = generate(MSG_VAR_DATA_IN_GROUP);
        assert!(result.is_ok(), "{result:?}");
    }

    #[test]
    fn test_var_data_in_group_emits_entry_accessors_and_end_offset() {
        let code = generate_ok(MSG_VAR_DATA_IN_GROUP);
        let entry = section(
            &code,
            "impl<'a> LegsEntryDecoder<'a>",
            "/// legs Group Encoder",
        );

        assert!(entry.contains("pub fn leg_qty(&self) -> u32"));
        assert!(entry.contains(
            "fn sbe_leg_tag_offset(&self) -> usize {\n        self.offset + self.block_length as usize\n"
        ));
        assert!(entry.contains("pub fn leg_tag(&self) -> &'a [u8]"));
        assert!(entry.contains("pub fn leg_tag_as_str(&self) -> &'a str"));
        assert!(entry.contains(
            "fn sbe_leg_note_offset(&self) -> usize {\n        let pos = self.sbe_leg_tag_offset();\n        pos + 2 + self.buffer.get_u16_le(pos) as usize\n"
        ));
        assert!(entry.contains("pub fn leg_note(&self) -> &'a [u8]"));
        assert!(entry.contains("let len = self.buffer.get_u8(pos) as usize;"));
        assert!(entry.contains(
            "pub fn end_offset(&self) -> usize {\n        let pos = self.sbe_leg_note_offset();\n        pos + 1 + self.buffer.get_u8(pos) as usize\n"
        ));
    }

    #[test]
    fn test_var_data_in_group_iterator_advances_by_entry_end() {
        let code = generate_ok(MSG_VAR_DATA_IN_GROUP);
        let group = section(
            &code,
            "impl<'a> LegsGroupDecoder<'a>",
            "/// legs Entry Decoder",
        );

        assert!(group.contains(
            "let entry = LegsEntryDecoder::wrap(self.buffer, self.offset, self.block_length);"
        ));
        assert!(group.contains("self.offset = entry.end_offset();"));
        assert!(!group.contains("self.offset += self.block_length as usize;"));
        assert!(
            group.contains(
                "pub fn end_offset(mut self) -> usize {\n        for _ in self.by_ref() {}\n        self.offset\n"
            ),
            "variable-stride group must walk its entries: {group}"
        );
    }

    #[test]
    fn test_message_var_data_after_variable_group_uses_group_offset() {
        let code = generate_ok(MSG_VAR_DATA_IN_GROUP);
        let decoder = section(&code, "impl<'a> QuoteDecoder<'a>", "impl<'a> SbeDecoder");

        assert!(
            decoder.contains("pos = quote::LegsGroupDecoder::wrap(self.buffer, pos).end_offset();")
        );
        assert!(decoder.contains(
            "fn sbe_comment_offset(&self) -> usize {\n        self.sbe_group_offset(1)\n"
        ));
        assert!(decoder.contains("pub fn comment(&self) -> &'a [u8]"));
    }

    #[test]
    fn test_nested_group_accessors_on_entry_decoder() {
        let code = generate_ok(MSG_NESTED_WITH_VAR_DATA);
        let entry = section(
            &code,
            "impl<'a> OrdersEntryDecoder<'a>",
            "/// fills Group Decoder",
        );

        // walker over the nested groups, based on the entry's wire block length
        assert!(entry.contains("fn sbe_group_offset(&self, index: usize) -> usize"));
        assert!(entry.contains("let mut pos = self.offset + self.block_length as usize;"));
        assert!(entry.contains("pos = FillsGroupDecoder::wrap(self.buffer, pos).end_offset();"));
        // nested accessor, unqualified (same module)
        assert!(entry.contains("pub fn fills(&self) -> FillsGroupDecoder<'a> {"));
        assert!(entry.contains("FillsGroupDecoder::wrap(self.buffer, self.sbe_group_offset(0))"));
        // var data after the nested group
        assert!(
            entry.contains(
                "fn sbe_memo_offset(&self) -> usize {\n        self.sbe_group_offset(1)\n"
            )
        );
        assert!(entry.contains("pub fn memo(&self) -> &'a [u8]"));

        // inner entry: var data straight after its fixed block
        let inner = section(
            &code,
            "impl<'a> FillsEntryDecoder<'a>",
            "/// orders Group Encoder",
        );
        assert!(inner.contains(
            "fn sbe_note_offset(&self) -> usize {\n        self.offset + self.block_length as usize\n"
        ));
        assert!(inner.contains("pub fn note(&self) -> &'a [u8]"));

        // message level: second group and trailer sit after the variable group
        let decoder = section(&code, "impl<'a> NestedDecoder<'a>", "impl<'a> SbeDecoder");
        assert!(
            decoder
                .contains("pos = nested::OrdersGroupDecoder::wrap(self.buffer, pos).end_offset();")
        );
        assert!(
            decoder
                .contains("nested::FlagsGroupDecoder::wrap(self.buffer, self.sbe_group_offset(1))")
        );
        assert!(decoder.contains(
            "fn sbe_trailer_offset(&self) -> usize {\n        self.sbe_group_offset(2)\n"
        ));
    }

    #[test]
    fn test_var_data_after_nested_group_no_longer_returns_unsupported() {
        let result = generate(
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
        );
        assert!(result.is_ok(), "{result:?}");
    }

    #[test]
    fn test_var_data_in_group_unknown_type_returns_unknown_type_err() {
        let err = generate(
            r#"
    <sbe:message name="Quote" id="1" blockLength="0">
        <group name="legs" id="10" dimensionType="groupSizeEncoding" blockLength="8">
            <field name="legId" id="11" type="uint64" offset="0"/>
            <data name="tag" id="12" type="noSuchEncoding"/>
        </group>
    </sbe:message>"#,
        )
        .expect_err("unknown var data type inside a group must be rejected");

        assert!(matches!(err, CodegenError::UnknownType { .. }), "{err:?}");
        let msg = err.to_string();
        assert!(msg.contains("noSuchEncoding"), "{msg}");
        assert!(msg.contains("Quote.legs.tag"), "{msg}");
    }

    #[test]
    fn test_var_data_in_group_length_int64_returns_unsupported_err() {
        let err = generate(
            r#"
    <sbe:message name="Quote" id="1" blockLength="0">
        <group name="legs" id="10" dimensionType="groupSizeEncoding" blockLength="8">
            <field name="legId" id="11" type="uint64" offset="0"/>
            <group name="fills" id="20" dimensionType="groupSizeEncoding" blockLength="8">
                <field name="fillId" id="21" type="uint64" offset="0"/>
                <data name="note" id="22" type="varDataEncoding64"/>
            </group>
        </group>
    </sbe:message>"#,
        )
        .expect_err("int64 length header inside a nested group must be rejected");

        assert!(matches!(err, CodegenError::Unsupported { .. }), "{err:?}");
        let msg = err.to_string();
        assert!(msg.contains("var data length encoding 'int64'"), "{msg}");
        assert!(
            msg.contains("message 'Quote', group 'legs', group 'fills', data 'note'"),
            "{msg}"
        );
    }

    #[test]
    fn test_var_data_in_group_entry_encoder_threads_cursor() {
        let code = generate_ok(MSG_VAR_DATA_IN_GROUP);

        let group = section(
            &code,
            "pub struct LegsGroupEncoder<'a>",
            "/// legs Entry Encoder",
        );
        assert!(
            group.contains(
                "Some(LegsEntryEncoder::wrap(&mut *self.buffer, offset, &mut *self.limit))"
            )
        );

        let entry = section(&code, "pub struct LegsEntryEncoder<'a>", "}\n\n}\n");
        assert!(entry.contains("    limit: &'a mut usize,\n"));
        assert!(entry.contains(
            "pub fn wrap(buffer: &'a mut [u8], offset: usize, limit: &'a mut usize) -> Self"
        ));
        assert!(entry.contains("pub fn set_leg_qty(&mut self, value: u32) -> &mut Self"));
        assert!(entry.contains("pub fn set_leg_tag(&mut self, value: &[u8]) -> &mut Self"));
        assert!(entry.contains("self.buffer.put_u16_le(*self.limit, len);"));
        assert!(entry.contains("let start = *self.limit + 2;"));
        assert!(entry.contains("*self.limit = start + value.len();"));
        assert!(entry.contains("pub fn set_leg_note(&mut self, value: &[u8]) -> &mut Self"));
        assert!(entry.contains("self.buffer.put_u8(*self.limit, len);"));

        // message-level var data after the group still uses the message cursor
        let encoder = section(&code, "pub struct QuoteEncoder<'a>", "/// Types for Quote");
        assert!(
            encoder.contains(
                "quote::LegsGroupEncoder::wrap(&mut *self.buffer, &mut self.limit, count)"
            )
        );
        assert!(encoder.contains("self.buffer.put_u16_le(self.limit, len);"));
    }

    #[test]
    fn test_nested_group_accessors_on_entry_encoder() {
        let code = generate_ok(MSG_NESTED_WITH_VAR_DATA);
        let entry = section(
            &code,
            "pub struct OrdersEntryEncoder<'a>",
            "/// fills Group Encoder",
        );

        assert!(
            entry.contains("pub fn fills_count(&mut self, count: u16) -> FillsGroupEncoder<'_>")
        );
        assert!(
            entry.contains("FillsGroupEncoder::wrap(&mut *self.buffer, &mut *self.limit, count)")
        );
        assert!(entry.contains("pub fn set_memo(&mut self, value: &[u8]) -> &mut Self"));

        let inner = section(
            &code,
            "pub struct FillsEntryEncoder<'a>",
            "/// flags Group Decoder",
        );
        assert!(inner.contains("pub fn set_note(&mut self, value: &[u8]) -> &mut Self"));
        assert!(inner.contains("self.buffer.put_u8(*self.limit, len);"));
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

    #[test]
    fn test_reader_emitted_only_for_messages_that_walk_the_wire() {
        // var data after flat groups: reader (var data still chains offsets)
        let code = generate_ok(MSG_GROUP_AND_VAR_DATA);
        assert!(code.contains("pub struct QuoteReader<'a> {"));
        // var data only: reader
        let code = generate_ok(MSG_ONLY_VAR_DATA);
        assert!(code.contains("pub struct BlobReader<'a> {"));
        // flat groups only: random access is already O(1), no reader
        let code = generate_ok(MSG_TWO_GROUPS);
        assert!(!code.contains("ListOrdersReader"));
        assert!(!code.contains("GroupReader"));
        // fixed fields only: no reader
        let code = generate_ok(MSG_FIXED_ONLY);
        assert!(!code.contains("PingReader"));
    }

    #[test]
    fn test_message_reader_reuses_decoder_validation_and_field_getters() {
        let code = generate_ok(MSG_GROUP_AND_VAR_DATA);
        let reader = section(&code, "impl<'a> QuoteReader<'a> {", "\n}\n\n");

        assert!(reader.contains("pos: offset + Self::BLOCK_LENGTH as usize,"));
        assert!(reader.contains("let decoder = QuoteDecoder::decode(buffer)?;"));
        assert!(reader.contains("decoder.acting_version,"));
        // same fixed-field getter as the decoder
        assert!(reader.contains(
            "pub fn qty(&self) -> u64 {\n        self.buffer.get_u64_le(self.offset + 0)"
        ));
        // flat group: existing decoder, cursor stepped in O(1)
        assert!(reader.contains("pub fn legs(&mut self) -> quote::LegsGroupDecoder<'a> {"));
        assert!(
            reader.contains("let group = quote::LegsGroupDecoder::wrap(self.buffer, self.pos);")
        );
        assert!(reader.contains(
            "self.pos = self.sbe_bounded(Some(group.end_offset()), \"repeating group 'legs'\");"
        ));
        // var data read once at the cursor, in schema order
        assert!(reader.contains("self.sbe_advance_to(1, \"var data field 'label'\");"));
        assert!(reader.contains("self.sbe_advance_to(2, \"var data field 'payload'\");"));
        assert!(reader.contains("pub fn finish(mut self) -> usize {\n        self.sbe_skip_to(Self::SBE_VAR_PARTS);\n        MessageHeader::ENCODED_LENGTH + (self.pos - self.offset)"));
        assert!(reader.contains("const SBE_VAR_PARTS: u16 = 3;"));
        // no group readers for flat groups
        assert!(!code.contains("LegsGroupReader"));
    }

    #[test]
    fn test_variable_groups_get_readers_recursively() {
        let code = generate_ok(MSG_NESTED_WITH_VAR_DATA);

        let reader = section(&code, "impl<'a> NestedReader<'a> {", "\n}\n\n");
        assert!(reader.contains("pub fn orders(&mut self) -> nested::OrdersGroupReader<'_, 'a> {"));
        assert!(reader.contains("nested::OrdersGroupReader::wrap(self.buffer, &mut self.pos)"));
        assert!(reader.contains("pub fn flags(&mut self) -> nested::FlagsGroupDecoder<'a> {"));
        assert!(reader.contains("pub fn trailer(&mut self) -> &'a [u8] {"));

        // group reader borrows the cursor and walks leftovers on drop
        assert!(code.contains("pub struct OrdersGroupReader<'r, 'a> {"));
        assert!(
            code.contains("pub fn next_entry(&mut self) -> Option<OrdersEntryReader<'_, 'a>> {")
        );
        let drop = section(
            &code,
            "impl Drop for OrdersGroupReader<'_, '_> {",
            "\n}\n\n",
        );
        assert!(drop.contains("if std::thread::panicking() {"));
        assert!(drop.contains(
            "let end = OrdersEntryDecoder::wrap(self.buffer, *self.pos, self.block_length).end_offset();"
        ));
        assert!(drop.contains("end <= self.buffer.len(),"));
        assert!(drop.contains("*self.pos = end;"));

        // entry reader: nested variable group then var data, borrowed cursor
        let entry = section(&code, "impl<'e, 'a> OrdersEntryReader<'e, 'a> {", "\n}\n\n");
        assert!(entry.contains("pub fn fills(&mut self) -> FillsGroupReader<'_, 'a> {"));
        assert!(entry.contains("FillsGroupReader::wrap(self.buffer, &mut *self.pos)"));
        assert!(entry.contains("self.sbe_advance_to(1, \"var data field 'memo'\");"));
        assert!(
            entry.contains(
                "let end = FillsGroupDecoder::wrap(self.buffer, *self.pos).end_offset();"
            )
        );
        assert!(entry.contains("const SBE_VAR_PARTS: u16 = 2;"));
        let entry_drop = section(
            &code,
            "impl Drop for OrdersEntryReader<'_, '_> {",
            "\n}\n\n",
        );
        assert!(entry_drop.contains("self.sbe_skip_to(Self::SBE_VAR_PARTS);"));

        // nested variable group gets its own reader pair; the flat one does not
        assert!(code.contains("pub struct FillsGroupReader<'r, 'a> {"));
        assert!(code.contains("pub struct FillsEntryReader<'e, 'a> {"));
        assert!(!code.contains("FlagsGroupReader"));
        assert!(!code.contains("FlagsEntryReader"));
    }

    /// Field, group and var data named like generated methods.
    const MSG_RESERVED_NAMES: &str = r#"
    <sbe:message name="Reserved" id="7" blockLength="4">
        <field name="wrap" id="1" type="uint32" offset="0"/>
        <group name="decode" id="10" dimensionType="groupSizeEncoding" blockLength="2">
            <field name="endOffset" id="11" type="uint16" offset="0"/>
        </group>
        <data name="finish" id="2" type="varStringEncoding"/>
    </sbe:message>"#;

    #[test]
    fn test_reserved_schema_names_are_suffixed_on_decoder_and_reader() {
        let code = generate_ok(MSG_RESERVED_NAMES);

        let decoder = section(&code, "impl<'a> ReservedDecoder<'a> {", "\n}\n\n");
        assert!(decoder.contains("pub fn wrap_(&self) -> u32 {"));
        assert!(decoder.contains("pub fn decode_(&self) -> reserved::DecodeGroupDecoder<'a> {"));
        assert!(decoder.contains("pub fn finish_(&self) -> &'a [u8] {"));
        assert!(decoder.contains("fn sbe_finish_offset(&self) -> usize {"));
        assert!(
            decoder.contains("Renamed from `finish` to avoid the generated `finish()` method.")
        );

        let reader = section(&code, "impl<'a> ReservedReader<'a> {", "\n}\n\n");
        assert!(reader.contains("pub fn wrap_(&self) -> u32 {"));
        assert!(reader.contains("pub fn decode_(&mut self) -> reserved::DecodeGroupDecoder<'a> {"));
        assert!(reader.contains("pub fn finish_(&mut self) -> &'a [u8] {"));
        assert!(reader.contains("pub fn finish(mut self) -> usize {"));
        assert!(reader.contains("pub fn decode(buffer: &'a [u8]) -> Result<Self, DecodeError> {"));

        // entry decoder keeps its own end_offset() next to the renamed getter
        let entry = section(&code, "impl<'a> DecodeEntryDecoder<'a> {", "\n}\n\n");
        assert!(entry.contains("pub fn end_offset_(&self) -> u16 {"));
        assert!(entry.contains("pub fn end_offset(&self) -> usize {"));

        // setters and group accessors on the encoder are prefixed, so untouched
        let encoder = section(&code, "impl<'a> ReservedEncoder<'a> {", "\n}\n\n");
        assert!(encoder.contains("pub fn set_wrap(&mut self, value: u32)"));
        assert!(encoder.contains("pub fn decode_count(&mut self, count: u16)"));
        assert!(encoder.contains("pub fn set_finish(&mut self, value: &[u8])"));
        assert!(encoder.contains("pub fn finish(mut self) -> usize {"));
    }
}
