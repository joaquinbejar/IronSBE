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

    /// Creates an unknown type error.
    pub fn unknown_type(type_name: impl Into<String>, field: impl Into<String>) -> Self {
        Self::UnknownType {
            type_name: type_name.into(),
            field: field.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_codegen_error_generation() {
        let err = CodegenError::generation("failed to generate code");
        let msg = err.to_string();
        assert!(msg.contains("failed to generate code"));
        assert!(msg.contains("generation error"));
    }

    #[test]
    fn test_codegen_error_unknown_type() {
        let err = CodegenError::unknown_type("MyType", "myField");
        let msg = err.to_string();
        assert!(msg.contains("MyType"));
        assert!(msg.contains("myField"));
        assert!(msg.contains("unknown type"));
    }

    #[test]
    fn test_codegen_error_debug() {
        let err = CodegenError::generation("test");
        let debug_str = format!("{:?}", err);
        assert!(debug_str.contains("Generation"));
    }
}
