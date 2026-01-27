//! Error types for schema parsing and validation.

use thiserror::Error;

/// Error type for schema parsing operations.
#[derive(Debug, Error)]
pub enum ParseError {
    /// XML parsing error.
    #[error("XML parsing error: {0}")]
    Xml(#[from] quick_xml::Error),

    /// Missing required attribute.
    #[error("missing required attribute '{attribute}' on element '{element}'")]
    MissingAttribute {
        /// Element name.
        element: String,
        /// Attribute name.
        attribute: String,
    },

    /// Invalid attribute value.
    #[error("invalid value '{value}' for attribute '{attribute}' on element '{element}'")]
    InvalidAttribute {
        /// Element name.
        element: String,
        /// Attribute name.
        attribute: String,
        /// Invalid value.
        value: String,
    },

    /// Unknown element encountered.
    #[error("unknown element '{element}' in context '{context}'")]
    UnknownElement {
        /// Element name.
        element: String,
        /// Parent context.
        context: String,
    },

    /// Unknown type reference.
    #[error("unknown type '{type_name}' referenced in field '{field}'")]
    UnknownType {
        /// Type name.
        type_name: String,
        /// Field name.
        field: String,
    },

    /// Duplicate definition.
    #[error("duplicate {kind} definition: '{name}'")]
    DuplicateDefinition {
        /// Kind of definition (type, message, etc.).
        kind: String,
        /// Name of the duplicate.
        name: String,
    },

    /// Invalid schema structure.
    #[error("invalid schema structure: {message}")]
    InvalidStructure {
        /// Error message.
        message: String,
    },

    /// IO error.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// UTF-8 decoding error.
    #[error("UTF-8 error: {0}")]
    Utf8(#[from] std::str::Utf8Error),
}

/// Error type for schema validation.
#[derive(Debug, Error)]
pub enum SchemaError {
    /// Parsing error.
    #[error("parse error: {0}")]
    Parse(#[from] ParseError),

    /// Type not found.
    #[error("type '{name}' not found")]
    TypeNotFound {
        /// Type name.
        name: String,
    },

    /// Message not found.
    #[error("message '{name}' not found")]
    MessageNotFound {
        /// Message name.
        name: String,
    },

    /// Invalid field offset.
    #[error(
        "invalid field offset: field '{field}' at offset {offset} overlaps with previous field"
    )]
    InvalidOffset {
        /// Field name.
        field: String,
        /// Invalid offset.
        offset: usize,
    },

    /// Block length mismatch.
    #[error(
        "block length mismatch for message '{message}': declared {declared}, calculated {calculated}"
    )]
    BlockLengthMismatch {
        /// Message name.
        message: String,
        /// Declared block length.
        declared: u16,
        /// Calculated block length.
        calculated: u16,
    },

    /// Circular type reference.
    #[error("circular type reference detected: {path}")]
    CircularReference {
        /// Path of the circular reference.
        path: String,
    },

    /// Invalid enum value.
    #[error("invalid enum value '{value}' for enum '{enum_name}'")]
    InvalidEnumValue {
        /// Enum name.
        enum_name: String,
        /// Invalid value.
        value: String,
    },

    /// Validation error.
    #[error("validation error: {message}")]
    Validation {
        /// Error message.
        message: String,
    },
}

impl ParseError {
    /// Creates a missing attribute error.
    pub fn missing_attr(element: impl Into<String>, attribute: impl Into<String>) -> Self {
        Self::MissingAttribute {
            element: element.into(),
            attribute: attribute.into(),
        }
    }

    /// Creates an invalid attribute error.
    pub fn invalid_attr(
        element: impl Into<String>,
        attribute: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        Self::InvalidAttribute {
            element: element.into(),
            attribute: attribute.into(),
            value: value.into(),
        }
    }

    /// Creates an unknown element error.
    pub fn unknown_element(element: impl Into<String>, context: impl Into<String>) -> Self {
        Self::UnknownElement {
            element: element.into(),
            context: context.into(),
        }
    }

    /// Creates a duplicate definition error.
    pub fn duplicate(kind: impl Into<String>, name: impl Into<String>) -> Self {
        Self::DuplicateDefinition {
            kind: kind.into(),
            name: name.into(),
        }
    }
}
