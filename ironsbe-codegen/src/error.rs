//! Error types for code generation.

use thiserror::Error;

/// Error type for code generation operations.
#[derive(Debug, Error)]
pub enum CodegenError {
    /// Schema parsing error.
    #[error("schema parse error: {0}")]
    Parse(#[from] ironsbe_schema::ParseError),

    /// Schema validation error.
    #[error("schema error: {0}")]
    Schema(#[from] ironsbe_schema::SchemaError),

    /// IO error.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// Code generation error.
    #[error("generation error: {message}")]
    Generation {
        /// Error message.
        message: String,
    },

    /// Unknown type reference.
    #[error("unknown type '{type_name}' in field '{field}'")]
    UnknownType {
        /// Type name.
        type_name: String,
        /// Field name.
        field: String,
    },
}

impl CodegenError {
    /// Creates a generation error with the given message.
    pub fn generation(message: impl Into<String>) -> Self {
        Self::Generation {
            message: message.into(),
        }
    }
}
