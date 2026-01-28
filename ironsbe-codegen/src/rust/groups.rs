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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_group_generator_new() {
        let generator = GroupGenerator::new();
        let _ = generator;
    }

    #[test]
    fn test_group_generator_default() {
        let generator = GroupGenerator;
        let _ = generator;
    }
}
