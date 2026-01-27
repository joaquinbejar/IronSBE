//! Group encoder/decoder code generation.
//!
//! This module is primarily used by the message generator for group handling.

/// Generator for group encoders and decoders.
pub struct GroupGenerator;

impl GroupGenerator {
    /// Creates a new group generator.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Default for GroupGenerator {
    fn default() -> Self {
        Self::new()
    }
}
