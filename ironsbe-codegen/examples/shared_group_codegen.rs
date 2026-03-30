//! Integration example: module-scoped group codegen for shared group names.
//!
//! Demonstrates that when multiple SBE messages share the same repeating group
//! name, the codegen produces per-message modules to avoid duplicate type
//! definitions. See <https://github.com/joaquinbejar/IronSBE/issues/5>.
//!
//! Run with:
//! ```sh
//! cargo run -p ironsbe-codegen --example shared_group_codegen
//! ```

fn rfq_schema() -> &'static str {
    r#"<?xml version="1.0" encoding="UTF-8"?>
<sbe:messageSchema xmlns:sbe="http://fixprotocol.io/2016/sbe"
                   package="rfq" id="10" version="1" byteOrder="littleEndian">
    <types>
        <type name="uint64" primitiveType="uint64"/>
        <type name="uint32" primitiveType="uint32"/>
        <type name="uint16" primitiveType="uint16"/>
    </types>

    <!-- Three messages sharing the same "quotes" group name -->

    <sbe:message name="CreateRfqResponse" id="21" blockLength="8">
        <field name="rfqId" id="1" type="uint64" offset="0"/>
        <group name="quotes" id="100" dimensionType="groupSizeEncoding" blockLength="16">
            <field name="price" id="200" type="uint64" offset="0"/>
            <field name="quantity" id="201" type="uint64" offset="8"/>
        </group>
    </sbe:message>

    <sbe:message name="GetRfqResponse" id="23" blockLength="8">
        <field name="rfqId" id="1" type="uint64" offset="0"/>
        <group name="quotes" id="100" dimensionType="groupSizeEncoding" blockLength="16">
            <field name="price" id="200" type="uint64" offset="0"/>
            <field name="quantity" id="201" type="uint64" offset="8"/>
        </group>
    </sbe:message>

    <sbe:message name="CancelRfqResponse" id="25" blockLength="8">
        <field name="rfqId" id="1" type="uint64" offset="0"/>
        <group name="quotes" id="100" dimensionType="groupSizeEncoding" blockLength="16">
            <field name="price" id="200" type="uint64" offset="0"/>
            <field name="quantity" id="201" type="uint64" offset="8"/>
        </group>
    </sbe:message>
</sbe:messageSchema>"#
}

fn main() {
    let code = ironsbe_codegen::generate_from_xml(rfq_schema()).expect("codegen failed");

    println!("=== Generated code ({} bytes) ===\n", code.len());
    println!("{code}");

    // --- structural assertions ---

    // Each message with groups must produce its own module.
    assert!(
        code.contains("pub mod create_rfq_response {"),
        "missing module for CreateRfqResponse"
    );
    assert!(
        code.contains("pub mod get_rfq_response {"),
        "missing module for GetRfqResponse"
    );
    assert!(
        code.contains("pub mod cancel_rfq_response {"),
        "missing module for CancelRfqResponse"
    );

    // Each module contains exactly one QuotesGroupDecoder definition.
    let group_decoder_count = code.matches("pub struct QuotesGroupDecoder").count();
    assert_eq!(
        group_decoder_count, 3,
        "expected 3 QuotesGroupDecoder definitions (one per message module), got {group_decoder_count}"
    );

    let entry_decoder_count = code.matches("pub struct QuotesEntryDecoder").count();
    assert_eq!(
        entry_decoder_count, 3,
        "expected 3 QuotesEntryDecoder definitions (one per message module), got {entry_decoder_count}"
    );

    // Group accessors in message decoders must use qualified paths.
    assert!(
        code.contains("create_rfq_response::QuotesGroupDecoder"),
        "CreateRfqResponse accessor must use qualified module path"
    );
    assert!(
        code.contains("get_rfq_response::QuotesGroupDecoder"),
        "GetRfqResponse accessor must use qualified module path"
    );
    assert!(
        code.contains("cancel_rfq_response::QuotesGroupDecoder"),
        "CancelRfqResponse accessor must use qualified module path"
    );

    // No top-level (unscoped) QuotesGroupDecoder outside of modules.
    // Every occurrence should be inside a `pub mod ... {` block or a qualified reference.
    let lines: Vec<&str> = code.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        if line.contains("pub struct QuotesGroupDecoder")
            || line.contains("pub struct QuotesEntryDecoder")
        {
            // Walk backwards to find enclosing module.
            let in_module = lines[..i]
                .iter()
                .rev()
                .any(|l| l.starts_with("pub mod ") && l.ends_with('{'));
            assert!(
                in_module,
                "line {}: group struct found outside a message module:\n  {line}",
                i + 1
            );
        }
    }

    println!("=== All assertions passed ===");
}
