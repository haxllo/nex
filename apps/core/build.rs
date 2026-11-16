//! Bakes the hand-written release notes for the crate's current version into
//! the binary, so the What's New view shows a curated page instead of whatever
//! the GitHub release body happens to contain.
//!
//! The alternative — `include_str!` on a version-specific path — fails to
//! compile the moment the version is bumped and the notes are not written yet.
//! That would turn "one edit per release" into "no build until you write
//! prose", so this reads the file if it exists and otherwise bakes an empty
//! string with a warning. An empty document is a state the view already
//! handles: it falls back to tips rather than rendering nothing.
//!
//! The release workflow refuses to publish without the notes file, so the
//! warning is for local builds and never reaches a published release.

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default());
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
    // apps/core -> repository root.
    let notes = manifest
        .join("..")
        .join("..")
        .join("docs")
        .join("releases")
        .join(format!("v{version}-notes.md"));

    println!("cargo:rerun-if-changed={}", notes.display());

    let body = match std::fs::read_to_string(&notes) {
        Ok(body) => body,
        Err(_) => {
            println!(
                "cargo:warning=no release notes at docs/releases/v{version}-notes.md; \
                 the What's New view will show the tips fallback for this build"
            );
            String::new()
        }
    };

    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap_or_default())
        .join("release_notes.md");
    // Only rewrite when the content changed: an unconditional write makes
    // every build look dirty to the compiler.
    if std::fs::read_to_string(&out).ok().as_deref() != Some(body.as_str()) {
        if let Err(error) = std::fs::write(&out, &body) {
            println!("cargo:warning=could not write baked release notes: {error}");
        }
    }
}