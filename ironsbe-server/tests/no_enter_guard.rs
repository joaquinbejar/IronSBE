//! Structural guard for issue #19.
//!
//! Holding a `Span::enter()` guard across `.await` on the multi-threaded runtime
//! races tracing-subscriber's per-thread span ref-counting and panics a worker in
//! `clone_span`, silently killing the SBE accept loop. The crate attaches spans
//! with `#[tracing::instrument]` / `Instrument::instrument` instead. This test
//! fails if a bare `.enter()` reappears anywhere under `src/`, so the anti-pattern
//! cannot be reintroduced unnoticed.
//!
//! Escape hatch: a line that genuinely needs a synchronous span guard (one that is
//! never held across `.await`) can opt out by appending `// allow: span-enter`.

use std::path::{Path, PathBuf};

/// Recursively collects every `.rs` file under `dir`.
fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => panic!("failed to read {}: {e}", dir.display()),
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => panic!("failed to read dir entry under {}: {e}", dir.display()),
        };
        let path = entry.path();
        if path.is_dir() {
            files.extend(rust_files(&path));
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            files.push(path);
        }
    }
    files
}

#[test]
fn no_span_enter_guard_in_src() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut offenders = Vec::new();

    for file in rust_files(&src) {
        let contents = match std::fs::read_to_string(&file) {
            Ok(contents) => contents,
            Err(e) => panic!("failed to read {}: {e}", file.display()),
        };
        for (index, line) in contents.lines().enumerate() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("//") || line.contains("// allow: span-enter") {
                continue;
            }
            if line.contains(".enter()") {
                offenders.push(format!("{}:{}", file.display(), index + 1));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "found `.enter()` span guard(s) under src/ (issue #19 anti-pattern; use \
         `#[tracing::instrument]` / `Instrument::instrument`, or append \
         `// allow: span-enter` if the guard is never held across `.await`):\n{}",
        offenders.join("\n")
    );
}
