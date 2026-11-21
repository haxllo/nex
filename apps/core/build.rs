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
    compile_glass_shaders();

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
/// Compile the glass pixel shader at build time, so the app does not pay for
/// it at every start (about 150 ms of CPU on the engine thread, and again
/// whenever the engine restarts).
///
/// The source is the reference implementation's own `PSGlass`: one pass that
/// bends, frosts, tints and lights the backdrop per pixel, and cuts the
/// silhouette with a one-pixel coverage ramp (`saturate(D * scale + 0.5)`)
/// instead of a bitmap mask or a window region. Those two are what made a
/// rounded corner quantise; a shader ramp cannot, at any radius or scale.
///
/// `D3DCompile` is called through a raw declaration because the alternative
/// (a build-dependency on the `windows` crate) would build that crate twice
/// for the host just to reach one function. Nothing is vendored: if the
/// compiler cannot be loaded the file is written empty and the app reports it
/// at start instead of failing the build.
fn compile_glass_shaders() {
    println!("cargo:rerun-if-changed=src/overlay/glass_ps.hlsl");
    let Ok(out) = std::env::var("OUT_DIR") else { return };
    let out = PathBuf::from(out);
    let src = std::fs::read("src/overlay/glass_ps.hlsl").unwrap_or_default();
    // Keep in step with the passes in `overlay::glass_ps`.
    let shaders = [
        ("ps_glass.dxbc", "PSGlass", "ps_5_0"),
        ("ps_crop.dxbc", "PSCrop", "ps_5_0"),
        ("ps_down.dxbc", "PSDown", "ps_5_0"),
        ("vs_main.dxbc", "VSMain", "vs_5_0"),
    ];
    for (file, entry, target) in shaders {
        let bytes = match compile(&src, entry, target, 1 << 15) {
            Ok(bytes) => bytes,
            Err(why) => {
                for line in format!("glass shader {entry} was not compiled at build time: {why}").lines() {
                    println!("cargo:warning={line}");
                }
                Vec::new()
            }
        };
        let path = out.join(file);
        if std::fs::read(&path).ok().as_deref() != Some(&bytes[..]) {
            let _ = std::fs::write(&path, &bytes);
        }
    }
}

#[cfg(not(windows))]
fn compile(_: &[u8], _: &str, _: &str, _: u32) -> Result<Vec<u8>, String> {
    Err("not a Windows host".into())
}

#[cfg(windows)]
fn compile(src: &[u8], entry: &str, target: &str, flags: u32) -> Result<Vec<u8>, String> {
    use std::ffi::c_void;

    #[repr(C)]
    struct Blob {
        vtbl: *const BlobVtbl,
    }
    #[repr(C)]
    struct BlobVtbl {
        query_interface: usize,
        add_ref: usize,
        release: unsafe extern "system" fn(*mut Blob) -> u32,
        buffer_pointer: unsafe extern "system" fn(*mut Blob) -> *mut c_void,
        buffer_size: unsafe extern "system" fn(*mut Blob) -> usize,
    }
    type D3DCompile = unsafe extern "system" fn(
        *const c_void,
        usize,
        *const u8,
        *const c_void,
        *const c_void,
        *const u8,
        *const u8,
        u32,
        u32,
        *mut *mut Blob,
        *mut *mut Blob,
    ) -> i32;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LoadLibraryExW(name: *const u16, file: *mut c_void, flags: u32) -> *mut c_void;
        fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
    }
    const LOAD_LIBRARY_SEARCH_SYSTEM32: u32 = 0x0000_0800;

    unsafe fn text(b: *mut Blob) -> Vec<u8> {
        if b.is_null() {
            return Vec::new();
        }
        unsafe {
            let v = &*(*b).vtbl;
            let bytes = std::slice::from_raw_parts((v.buffer_pointer)(b) as *const u8, (v.buffer_size)(b)).to_vec();
            (v.release)(b);
            bytes
        }
    }

    unsafe {
        let dll: Vec<u16> = "d3dcompiler_47.dll".encode_utf16().chain([0]).collect();
        let module = LoadLibraryExW(dll.as_ptr(), std::ptr::null_mut(), LOAD_LIBRARY_SEARCH_SYSTEM32);
        if module.is_null() {
            return Err("d3dcompiler_47.dll cannot be loaded".into());
        }
        let f = GetProcAddress(module, c"D3DCompile".as_ptr() as *const u8);
        if f.is_null() {
            return Err("D3DCompile is not in d3dcompiler_47.dll".into());
        }
        let d3d_compile: D3DCompile = std::mem::transmute(f);
        let (name, entry_z, target_z) = (b"glass_ps.hlsl ".as_slice(), format!("{entry} "), format!("{target} "));
        let (mut code, mut errors): (*mut Blob, *mut Blob) = (std::ptr::null_mut(), std::ptr::null_mut());
        let hr = d3d_compile(
            src.as_ptr() as *const c_void,
            src.len(),
            name.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            entry_z.as_ptr(),
            target_z.as_ptr(),
            flags,
            0,
            &mut code,
            &mut errors,
        );
        let message = String::from_utf8_lossy(&text(errors)).into_owned();
        if hr < 0 {
            return Err(format!("D3DCompile failed ({hr:#x}): {message}"));
        }
        let bytes = text(code);
        if bytes.is_empty() {
            return Err("D3DCompile returned no bytecode".into());
        }
        Ok(bytes)
    }
}
