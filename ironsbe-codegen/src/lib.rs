//! # IronSBE Codegen
//!
//! Code generation from SBE XML schemas.
//!
//! This crate provides:
//! - Rust code generation from SBE schemas
//! - Message encoder/decoder generation
//! - Type and enum generation
//! - Build script integration

pub mod error;
pub mod generator;
pub mod rust;

pub use error::CodegenError;
pub use generator::Generator;

/// Generates Rust code from an SBE XML schema string.
///
/// # Arguments
/// * `xml` - SBE XML schema content
///
/// # Returns
/// Generated Rust code as a string.
///
/// # Errors
/// Returns `CodegenError` if parsing fails, if the schema references an
/// unknown type from a `<data>` element, or if it uses a construct the
/// generator does not support yet (see [`CodegenError::Unsupported`]).
pub fn generate_from_xml(xml: &str) -> Result<String, CodegenError> {
    let schema = ironsbe_schema::parse_schema(xml)?;
    let ir = ironsbe_schema::SchemaIr::from_schema(&schema);
    let generator = Generator::new(&ir);
    generator.generate()
}

/// Generates Rust code from an SBE XML schema file.
///
/// # Arguments
/// * `path` - Path to the SBE XML schema file
///
/// # Returns
/// Generated Rust code as a string.
///
/// # Errors
/// Returns `CodegenError` if reading, parsing, or generation fails.
pub fn generate_from_file(path: &std::path::Path) -> Result<String, CodegenError> {
    let xml = std::fs::read_to_string(path)?;
    generate_from_xml(&xml)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_from_xml_var_data_in_group_returns_unsupported() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<sbe:messageSchema xmlns:sbe="http://fixprotocol.io/2016/sbe"
                   package="test" id="1" version="1" byteOrder="littleEndian">
    <types>
        <type name="uint64" primitiveType="uint64"/>
        <composite name="varStringEncoding">
            <type name="length" primitiveType="uint16"/>
            <type name="varData" primitiveType="uint8" length="0"/>
        </composite>
    </types>
    <sbe:message name="Quote" id="1" blockLength="0">
        <group name="legs" id="10" dimensionType="groupSizeEncoding" blockLength="8">
            <field name="legId" id="11" type="uint64" offset="0"/>
            <data name="note" id="12" type="varStringEncoding"/>
        </group>
    </sbe:message>
</sbe:messageSchema>"#;

        let err = generate_from_xml(xml).expect_err("var data in group must fail codegen");
        assert!(matches!(err, CodegenError::Unsupported { .. }), "{err:?}");
        assert!(err.to_string().contains("<data> inside repeating group"));
    }

    #[test]
    fn test_generate_from_xml_message_level_var_data_succeeds() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<sbe:messageSchema xmlns:sbe="http://fixprotocol.io/2016/sbe"
                   package="test" id="1" version="1" byteOrder="littleEndian">
    <types>
        <type name="uint64" primitiveType="uint64"/>
        <composite name="varStringEncoding">
            <type name="length" primitiveType="uint16"/>
            <type name="varData" primitiveType="uint8" length="0"/>
        </composite>
    </types>
    <sbe:message name="Example" id="1" blockLength="8">
        <field name="qty" id="1" type="uint64" offset="0"/>
        <data name="label" id="2" type="varStringEncoding"/>
    </sbe:message>
</sbe:messageSchema>"#;

        let code = generate_from_xml(xml).expect("codegen failed");
        assert!(code.contains("pub fn label(&self) -> &'a [u8]"));
        assert!(code.contains("pub fn set_label(&mut self, value: &[u8]) -> &mut Self"));
    }
}
