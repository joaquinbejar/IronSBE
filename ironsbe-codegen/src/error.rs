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

    /// Schema construct the generator cannot emit correct code for yet.
    ///
    /// Emitted instead of silently producing an incomplete module, so a
    /// consumer never gets a codec that compiles but cannot round-trip its
    /// own messages.
    #[error("unsupported: {feature} ({context})")]
    Unsupported {
        /// Human-readable name of the unsupported construct,
        /// e.g. `<data> inside repeating group`.
        feature: String,
        /// Where it was found, e.g. `message 'Quote', group 'legs'`.
        context: String,
    },
}

impl CodegenError {
    /// Creates a generation error with the given message.
    #[cold]
    pub fn generation(message: impl Into<String>) -> Self {
        Self::Generation {
            message: message.into(),
        }
    }

    /// Creates an unknown type error.
    #[cold]
    pub fn unknown_type(type_name: impl Into<String>, field: impl Into<String>) -> Self {
        Self::UnknownType {
            type_name: type_name.into(),
            field: field.into(),
        }
    }

    /// Creates an unsupported-construct error.
    ///
    /// # Arguments
    /// * `feature` - Name of the unsupported schema construct
    /// * `context` - Message / group where it was found
    #[cold]
    pub fn unsupported(feature: impl Into<String>, context: impl Into<String>) -> Self {
        Self::Unsupported {
            feature: feature.into(),
            context: context.into(),
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

    #[test]
    fn test_codegen_error_unsupported_display_contains_feature_and_context() {
        let err = CodegenError::unsupported(
            "<data> inside repeating group",
            "message 'Quote', group 'legs'",
        );
        let msg = err.to_string();
        assert!(msg.starts_with("unsupported: "), "got: {msg}");
        assert!(msg.contains("<data> inside repeating group"));
        assert!(msg.contains("message 'Quote', group 'legs'"));
        assert!(matches!(err, CodegenError::Unsupported { .. }));
    }
}
