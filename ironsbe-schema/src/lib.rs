//! # IronSBE Schema
//!
//! SBE XML schema parser and type definitions.
//!
//! This crate provides:
//! - XML schema parsing from FIX SBE specifications
//! - Type definitions for schema elements
//! - Schema validation
//! - Intermediate representation for code generation

pub mod error;
pub mod ir;
pub mod messages;
pub mod parser;
pub mod types;
pub mod validation;

pub use error::{ParseError, SchemaError};
pub use ir::SchemaIr;
pub use messages::{DataFieldDef, FieldDef, GroupDef, MessageDef};
pub use parser::parse_schema;
pub use types::{
    ByteOrder, CompositeDef, CompositeField, EnumDef, EnumValue, Presence, PrimitiveDef,
    PrimitiveType, Schema, SetChoice, SetDef, TypeDef,
};
