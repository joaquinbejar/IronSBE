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
use crate::rust::var_data::{
    VarDataInfo, end_offset_parts, generate_var_data_getter, generate_var_data_setter,
    resolve_var_data,
};

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
            let var_data = resolve_var_data(self.ir, &context, &msg.name, &msg.var_data)?;
            let groups = msg
                .groups
                .iter()
                .map(|g| GroupLayout::resolve(self.ir, &context, &msg.name, g))
                .collect::<Result<Vec<_>, _>>()?;

            output.push_str(&self.generate_decoder(msg, &groups, &var_data));
            output.push_str(&self.generate_encoder(msg, &groups, &var_data));

            // Generate group decoders and encoders in a message-scoped module
            if !groups.is_empty() {
                let mod_name = to_snake_case(&msg.name);
                output.push_str(&format!("/// Types for {} repeating groups.\n", msg.name));
                output.push_str(&format!("pub mod {} {{\n", mod_name));
                output.push_str("    use super::*;\n\n");
                for layout in &groups {
                    output.push_str(&generate_group_decoder(self.ir, layout));
                    output.push_str(&generate_group_encoder(self.ir, layout));
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
            output.push_str(&generate_field_getter(self.ir, field));
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
            output.push_str(&generate_field_setter(self.ir, field));
        }

        // Group encoder accessors (lend the write cursor to the group encoder)
        let mod_name = to_snake_case(&msg.name);
        for layout in groups {
            output.push_str(&generate_group_encoder_accessor(
                &layout.group.name,
                &format!("{mod_name}::{}", layout.group.encoder_name()),
                "&mut self.limit",
            ));
        }

        // Var data setters (append at the write cursor)
        for info in var_data {
            output.push_str(&generate_var_data_setter(info, "self.limit"));
        }

        output.push_str("}\n\n");

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

        // Encoder lends its cursor to each group encoder instead of pre-advancing it.
        assert!(
            !code.contains("self.limit += GroupHeader::ENCODED_LENGTH"),
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
        assert!(decoder_impl.contains("let pos = self.payload_offset();"));
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
            !entry.contains("fn group_offset("),
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
            "fn leg_tag_offset(&self) -> usize {\n        self.offset + self.block_length as usize\n"
        ));
        assert!(entry.contains("pub fn leg_tag(&self) -> &'a [u8]"));
        assert!(entry.contains("pub fn leg_tag_as_str(&self) -> &'a str"));
        assert!(entry.contains(
            "fn leg_note_offset(&self) -> usize {\n        let pos = self.leg_tag_offset();\n        pos + 2 + self.buffer.get_u16_le(pos) as usize\n"
        ));
        assert!(entry.contains("pub fn leg_note(&self) -> &'a [u8]"));
        assert!(entry.contains("let len = self.buffer.get_u8(pos) as usize;"));
        assert!(entry.contains(
            "pub fn end_offset(&self) -> usize {\n        let pos = self.leg_note_offset();\n        pos + 1 + self.buffer.get_u8(pos) as usize\n"
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
        assert!(
            decoder.contains("fn comment_offset(&self) -> usize {\n        self.group_offset(1)\n")
        );
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
        assert!(entry.contains("fn group_offset(&self, index: usize) -> usize"));
        assert!(entry.contains("let mut pos = self.offset + self.block_length as usize;"));
        assert!(entry.contains("pos = FillsGroupDecoder::wrap(self.buffer, pos).end_offset();"));
        // nested accessor, unqualified (same module)
        assert!(entry.contains("pub fn fills(&self) -> FillsGroupDecoder<'a> {"));
        assert!(entry.contains("FillsGroupDecoder::wrap(self.buffer, self.group_offset(0))"));
        // var data after the nested group
        assert!(entry.contains("fn memo_offset(&self) -> usize {\n        self.group_offset(1)\n"));
        assert!(entry.contains("pub fn memo(&self) -> &'a [u8]"));

        // inner entry: var data straight after its fixed block
        let inner = section(
            &code,
            "impl<'a> FillsEntryDecoder<'a>",
            "/// orders Group Encoder",
        );
        assert!(inner.contains(
            "fn note_offset(&self) -> usize {\n        self.offset + self.block_length as usize\n"
        ));
        assert!(inner.contains("pub fn note(&self) -> &'a [u8]"));

        // message level: second group and trailer sit after the variable group
        let decoder = section(&code, "impl<'a> NestedDecoder<'a>", "impl<'a> SbeDecoder");
        assert!(
            decoder
                .contains("pos = nested::OrdersGroupDecoder::wrap(self.buffer, pos).end_offset();")
        );
        assert!(
            decoder.contains("nested::FlagsGroupDecoder::wrap(self.buffer, self.group_offset(1))")
        );
        assert!(
            decoder.contains("fn trailer_offset(&self) -> usize {\n        self.group_offset(2)\n")
        );
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
}
