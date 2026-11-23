//! Accessor naming for generated codecs.
//!
//! Schema names become snake_case methods on the generated types, next to
//! the methods the generator itself defines (`wrap`, `decode`, `finish`,
//! `end_offset`, ...). A field, group or var data whose accessor would
//! collide with one of those is emitted with a trailing underscore instead
//! (`finish_()`), so every valid schema still compiles and the rename shows
//! up in the accessor's docs. Setters (`set_<name>`, `<group>_count`) and
//! the string variants (`<name>_as_str`) carry a prefix or suffix already
//! and cannot collide, so they keep the plain name.

/// Methods every message decoder and reader defines: a field, group or var
/// data accessor with one of these names is renamed on both.
pub(crate) const MESSAGE_RESERVED: &[&str] = &[
    "wrap",
    "decode",
    "validate_header",
    "encoded_length",
    "acting_version",
    "acting_block_length",
    "wrap_acting",
    "finish",
];

/// Methods every entry decoder and reader defines: a field, nested group or
/// var data accessor with one of these names is renamed on both.
pub(crate) const ENTRY_RESERVED: &[&str] = &["wrap", "end_offset"];

/// `code`, a group entry decoder or reader (its struct, then its impl),
/// without the `acting_version` and `block_length` fields its impl never
/// reads: a dead field is an error under a downstream `-D warnings`, and a
/// metadata accessor would take a name a schema field may already use. The
/// private `wrap` keeps both parameters, unused ones prefixed with `_`.
pub(crate) fn prune_entry_metadata(code: String) -> String {
    let Some(impl_at) = code.find("\nimpl") else {
        return code;
    };
    let mut code = code;
    for field in ["acting_version", "block_length"] {
        let read = format!("self.{field}");
        if code.get(impl_at..).is_some_and(|body| body.contains(&read)) {
            continue;
        }
        code = code.replacen(&format!("    {field}: u16,\n"), "", 1);
        code = code.replacen(&format!("{field}: u16"), &format!("_{field}: u16"), 1);
        code = code
            .lines()
            .map(|line| {
                if line.trim_start().starts_with("Self { buffer") {
                    line.replace(&format!(", {field}"), "")
                } else {
                    line.to_owned()
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
    }
    code
}

/// Returns the method name for a snake_case schema accessor on a host that
/// defines `reserved`: the name itself, or the name with a trailing
/// underscore when it collides.
#[must_use]
pub(crate) fn accessor_name(snake: &str, reserved: &[&str]) -> String {
    if reserved.contains(&snake) {
        format!("{snake}_")
    } else {
        snake.to_string()
    }
}

/// Returns the doc line explaining a renamed accessor, or an empty string
/// when `accessor` is the plain snake_case name.
#[must_use]
pub(crate) fn renamed_note(snake: &str, accessor: &str) -> String {
    if accessor == snake {
        String::new()
    } else {
        format!(
            "    ///\n    /// Renamed from `{snake}` to avoid the generated `{snake}()` method.\n"
        )
    }
}

/// The position `base + offset` as generated code, or `base` alone when the
/// offset is zero, so the output carries no no-op `+ 0` (clippy's
/// `identity_op` under a downstream `-D warnings`).
pub(crate) fn plus(base: &str, offset: impl std::fmt::Display) -> String {
    let offset = offset.to_string();
    if offset == "0" {
        base.to_owned()
    } else {
        format!("{base} + {offset}")
    }
}

#[cfg(test)]
mod tests {
    /// IronSBE#80 review: an entry whose impl never reads its metadata
    /// loses those fields, and keeps them when it reads them.
    #[test]
    fn test_prune_entry_metadata() {
        let unread = "pub struct E<'a> {\n    buffer: &'a [u8],\n    offset: usize,\n    block_length: u16,\n    acting_version: u16,\n}\n\nimpl<'a> E<'a> {\n    fn wrap(buffer: &'a [u8], offset: usize, block_length: u16, acting_version: u16) -> Self {\n        Self { buffer, offset, block_length, acting_version }\n    }\n}\n".to_owned();
        let pruned = super::prune_entry_metadata(unread);
        assert!(!pruned.contains("    acting_version: u16,\n"), "{pruned}");
        assert!(!pruned.contains("    block_length: u16,\n"), "{pruned}");
        assert!(
            pruned.contains("_block_length: u16, _acting_version: u16"),
            "{pruned}"
        );
        assert!(pruned.contains("Self { buffer, offset }"), "{pruned}");
        let read = "pub struct E {\n    acting_version: u16,\n}\n\nimpl E {\n    fn v(&self) -> u16 { self.acting_version }\n}\n".to_owned();
        assert_eq!(
            super::prune_entry_metadata(read.clone())
                .matches("acting_version: u16")
                .count(),
            1
        );
    }

    use super::*;

    #[test]
    fn test_accessor_name_keeps_plain_names() {
        assert_eq!(accessor_name("leg_qty", MESSAGE_RESERVED), "leg_qty");
        assert_eq!(accessor_name("finish", ENTRY_RESERVED), "finish");
    }

    #[test]
    fn test_accessor_name_suffixes_reserved_names() {
        assert_eq!(accessor_name("finish", MESSAGE_RESERVED), "finish_");
        assert_eq!(accessor_name("wrap", MESSAGE_RESERVED), "wrap_");
        assert_eq!(accessor_name("decode", MESSAGE_RESERVED), "decode_");
        assert_eq!(accessor_name("end_offset", ENTRY_RESERVED), "end_offset_");
        assert_eq!(accessor_name("wrap", ENTRY_RESERVED), "wrap_");
    }

    #[test]
    fn test_renamed_note_only_for_renames() {
        assert!(renamed_note("qty", "qty").is_empty());
        let note = renamed_note("finish", "finish_");
        assert!(note.contains("Renamed from `finish` to avoid the generated `finish()` method."));
    }
}
