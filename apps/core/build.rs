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
//! It also bakes an optional picture for the page, if one happens to exist at
//! `docs/releases/assets/v<version>-hero.webp` or `whats-new-hero.webp`. No
//! release needs one: the page is the heading and the text, and a missing
//! picture is the normal case, not a warning. Baking it rather than fetching
//! it keeps the page whole when the network is slow; a release that wants a
//! banner gets the same treatment, and one that does not notices nothing.
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
    write_if_changed(&out, &body, "the baked What's New document");

    // The page's banner. Baked the same way and for the same reason as the
    // text: a release page that fetches its own picture shows a hole whenever
    // the network is slow, and the picture has to match the notes beside it.
    // A data URI keeps the asset pipeline out of it entirely - no protocol
    // route, no cache directory, nothing to invalidate. 40 KB of WebP is about
    // 55 KB of base64, which is a fair price for one image per release.
    let assets = manifest.join("..").join("..").join("docs").join("releases").join("assets");
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
    let candidates = [
        assets.join(format!("v{version}-hero.webp")),
        assets.join("whats-new-hero.webp"),
    ];
    for candidate in &candidates {
        println!("cargo:rerun-if-changed={}", candidate.display());
    }
    let hero = match candidates.iter().find_map(|path| std::fs::read(path).ok()) {
        Some(bytes) => format!("data:image/webp;base64,{}", base64(&bytes)),
        None => String::new(),
    };
    let hero_out = PathBuf::from(std::env::var("OUT_DIR").unwrap_or_default()).join("whats_new_hero.txt");
    write_if_changed(&hero_out, &hero, "the baked What's New banner");
}

fn write_if_changed(path: &PathBuf, body: &str, what: &str) {
    if std::fs::read_to_string(path).ok().as_deref() != Some(body) {
        if let Err(error) = std::fs::write(path, body) {
            println!("cargo:warning=could not write {what}: {error}");
        }
    }
}

/// Standard base64, written out rather than pulled in: a build script with no
/// dependencies stays cheap, and this is the whole alphabet.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[(triple >> 18) as usize & 63] as char);
        out.push(ALPHABET[(triple >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { ALPHABET[(triple >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { ALPHABET[triple as usize & 63] as char } else { '=' });
    }
    out
}