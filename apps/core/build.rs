//! Bakes the hand-written What's New document into the binary, so the view
//! shows a curated page instead of whatever a generated body happens to
//! contain.
//!
//! Two separate documents, on purpose:
//!
//! * `docs/whats-new.md` — what the person using Nex reads in the app. Edited
//!   before each release. It carries its own format instructions in HTML
//!   comments, stripped at render time by `whats_new::notes_markdown`.
//! * `docs/releases/v<version>-notes.md` — the GitHub release page, including
//!   the commit log. Not used by the app at all.
//!
//! Stripping the comments here rather than in the app was considered and
//! rejected: a build script is compiled without the test harness, so the
//! stripper's tests would never run and the rules would be unverified.
//!
//! The document is read if present and otherwise baked empty with a warning.
//! An empty document is a state the view already handles by falling back to
//! tips, and `include_str!` on a missing file would instead break every build.

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default());
    // apps/core -> repository root.
    let doc = manifest
        .join("..")
        .join("..")
        .join("docs")
        .join("whats-new.md");

    println!("cargo:rerun-if-changed={}", doc.display());

    let body = match std::fs::read_to_string(&doc) {
        Ok(body) => body,
        Err(_) => {
            println!(
                "cargo:warning=no What's New document at docs/whats-new.md; \
                 the view will show the tips fallback for this build"
            );
            String::new()
        }
    };

    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap_or_default()).join("whats_new.md");
    // Only rewrite when the content changed: an unconditional write makes
    // every build look dirty to the compiler.
    if std::fs::read_to_string(&out).ok().as_deref() != Some(body.as_str()) {
        if let Err(error) = std::fs::write(&out, &body) {
            println!("cargo:warning=could not write the baked What's New document: {error}");
        }
    }
}