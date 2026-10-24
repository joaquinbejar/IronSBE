//! Configuration for the code generator.

/// Options that change what the generator emits.
///
/// The default reproduces the generator's historical output, so
/// `GeneratorOptions::default()` is always a safe choice.
///
/// # Examples
/// ```
/// use ironsbe_codegen::GeneratorOptions;
///
/// // Emit only the non-panicking `try_set_*` var data setters.
/// let options = GeneratorOptions::default().with_panicking_setters(false);
/// assert!(!options.panicking_setters());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct GeneratorOptions {
    panicking_setters: bool,
}

impl GeneratorOptions {
    /// Sets whether the panicking `set_<field>` var data setters are
    /// emitted next to the fallible `try_set_<field>` ones.
    ///
    /// Defaults to `true`. Pass `false` for crates that deny
    /// `clippy::panic`: generated encoders then contain no `panic!`.
    #[must_use = "builders do nothing unless the options are passed to the generator"]
    pub const fn with_panicking_setters(mut self, enabled: bool) -> Self {
        self.panicking_setters = enabled;
        self
    }

    /// Returns whether the panicking `set_<field>` var data setters are
    /// emitted.
    #[must_use]
    pub const fn panicking_setters(&self) -> bool {
        self.panicking_setters
    }
}

impl Default for GeneratorOptions {
    fn default() -> Self {
        Self {
            panicking_setters: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generator_options_default_emits_panicking_setters() {
        assert!(GeneratorOptions::default().panicking_setters());
    }

    #[test]
    fn test_generator_options_with_panicking_setters_false_disables_them() {
        let options = GeneratorOptions::default().with_panicking_setters(false);
        assert!(!options.panicking_setters());
    }
}
