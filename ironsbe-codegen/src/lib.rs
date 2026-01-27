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
/// Returns `CodegenError` if parsing or generation fails.
pub fn generate_from_xml(xml: &str) -> Result<String, CodegenError> {
    let schema = ironsbe_schema::parse_schema(xml)?;
    let ir = ironsbe_schema::SchemaIr::from_schema(&schema);
    let generator = Generator::new(&ir);
    Ok(generator.generate())
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
