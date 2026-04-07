//! Integration example: round-trip codegen for repeating groups.
//!
//! Generates code for a message with a repeating group, then verifies that:
//! 1. Entry decoder field offsets are correct (not all zero).
//! 2. Group encoder and entry encoder structs are emitted.
//! 3. Parent message encoder has a group accessor.
//! 4. Field setters in the entry encoder use the correct offsets.
//!
//! See <https://github.com/joaquinbejar/IronSBE/issues/9>.
//!
//! Run with:
//! ```sh
//! cargo run -p ironsbe-codegen --example group_roundtrip_codegen
//! ```

fn order_schema() -> &'static str {
    r#"<?xml version="1.0" encoding="UTF-8"?>
<sbe:messageSchema xmlns:sbe="http://fixprotocol.io/2016/sbe"
                   package="orders" id="10" version="1" byteOrder="littleEndian">
    <types>
        <type name="uint64" primitiveType="uint64"/>
        <type name="uint32" primitiveType="uint32"/>
        <type name="uint16" primitiveType="uint16"/>
        <type name="uint8" primitiveType="uint8"/>
    </types>

    <!-- Message with fixed fields + a repeating group whose fields omit offsets -->
    <sbe:message name="ListOrdersResponse" id="19" blockLength="8">
        <field name="requestId" id="1" type="uint64" offset="0"/>
        <group name="orders" id="100" dimensionType="groupSizeEncoding" blockLength="29">
            <field name="orderId" id="10" type="uint64" offset="0"/>
            <field name="instrumentId" id="11" type="uint32"/>
            <field name="price" id="12" type="uint64"/>
            <field name="quantity" id="13" type="uint64"/>
            <field name="side" id="14" type="uint8"/>
        </group>
    </sbe:message>
</sbe:messageSchema>"#
}

fn main() {
    let code = ironsbe_codegen::generate_from_xml(order_schema()).expect("codegen failed");

    println!("=== Generated code ({} bytes) ===\n", code.len());
    println!("{code}");

    // --- 1. Entry decoder offsets ---

    let entry_decoder_pos = code
        .find("impl<'a> OrdersEntryDecoder<'a>")
        .expect("OrdersEntryDecoder impl not found");
    let decoder_section = &code[entry_decoder_pos..];

    // orderId at offset 0
    assert!(
        decoder_section.contains("self.offset + 0)"),
        "orderId getter should read at offset 0"
    );
    // instrumentId at offset 8 (after uint64)
    assert!(
        decoder_section.contains("self.offset + 8)"),
        "instrumentId getter should read at offset 8, not 0"
    );
    // price at offset 12 (after uint64 + uint32)
    assert!(
        decoder_section.contains("self.offset + 12)"),
        "price getter should read at offset 12"
    );
    // quantity at offset 20 (after uint64 + uint32 + uint64)
    assert!(
        decoder_section.contains("self.offset + 20)"),
        "quantity getter should read at offset 20"
    );
    // side at offset 28 (after uint64 + uint32 + uint64 + uint64)
    assert!(
        decoder_section.contains("self.offset + 28)"),
        "side getter should read at offset 28"
    );
    println!("=== [PASS] Entry decoder offsets are correct ===\n");

    // --- 2. Group encoder + entry encoder exist ---

    assert!(
        code.contains("pub struct OrdersGroupEncoder"),
        "missing OrdersGroupEncoder struct"
    );
    assert!(
        code.contains("pub struct OrdersEntryEncoder"),
        "missing OrdersEntryEncoder struct"
    );
    println!("=== [PASS] Group/entry encoder structs emitted ===\n");

    // --- 3. Group encoder API ---

    assert!(
        code.contains("fn next_entry(&mut self)"),
        "missing next_entry on group encoder"
    );
    assert!(
        code.contains("BLOCK_LENGTH: u16 = 29"),
        "group encoder should have BLOCK_LENGTH = 29"
    );
    assert!(
        code.contains("fn wrap(buffer: &'a mut [u8], offset: usize, count: u16)"),
        "group encoder should have wrap(buffer, offset, count)"
    );
    println!("=== [PASS] Group encoder API is correct ===\n");

    // --- 4. Entry encoder field setters with correct offsets ---

    let entry_encoder_pos = code
        .find("impl<'a> OrdersEntryEncoder<'a>")
        .expect("OrdersEntryEncoder impl not found");
    let encoder_section = &code[entry_encoder_pos..];

    assert!(
        encoder_section.contains("fn set_order_id(&mut self, value: u64)"),
        "missing set_order_id"
    );
    assert!(
        encoder_section.contains("fn set_instrument_id(&mut self, value: u32)"),
        "missing set_instrument_id"
    );
    assert!(
        encoder_section.contains("fn set_price(&mut self, value: u64)"),
        "missing set_price"
    );
    assert!(
        encoder_section.contains("fn set_quantity(&mut self, value: u64)"),
        "missing set_quantity"
    );
    assert!(
        encoder_section.contains("fn set_side(&mut self, value: u8)"),
        "missing set_side"
    );

    // Verify offsets in setters
    assert!(
        encoder_section.contains("self.offset + 0,"),
        "set_order_id should write at offset 0"
    );
    assert!(
        encoder_section.contains("self.offset + 8,"),
        "set_instrument_id should write at offset 8"
    );
    assert!(
        encoder_section.contains("self.offset + 12,"),
        "set_price should write at offset 12"
    );
    assert!(
        encoder_section.contains("self.offset + 20,"),
        "set_quantity should write at offset 20"
    );
    assert!(
        encoder_section.contains("self.offset + 28,"),
        "set_side should write at offset 28"
    );
    println!("=== [PASS] Entry encoder setter offsets are correct ===\n");

    // --- 5. Parent message encoder has group accessor ---

    assert!(
        code.contains("fn orders_count(&mut self, count: u16)"),
        "missing orders_count accessor on parent encoder"
    );
    assert!(
        code.contains("list_orders_response::OrdersGroupEncoder::wrap"),
        "parent encoder should delegate to module-qualified OrdersGroupEncoder::wrap"
    );
    println!("=== [PASS] Parent encoder exposes group accessor ===\n");

    println!("=== All assertions passed ===");
}
