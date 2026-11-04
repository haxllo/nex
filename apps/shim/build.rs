fn main() {
    println!("cargo:rerun-if-changed=../assets/nex.ico");
    println!("cargo:rerun-if-changed=build.rs");

    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "windows" {
        return;
    }

    // Its own package so the version resource stays distinct from the UI and
    // indexer binaries (a package build script stamps every binary it builds).
    let mut res = winres::WindowsResource::new();
    if std::path::Path::new("../assets/nex.ico").exists() {
        res.set_icon("../assets/nex.ico");
    }
    res.set("FileDescription", "Nex Launcher");
    res.set("InternalName", "NexLauncher");
    res.set("OriginalFilename", "Nex.exe");
    res.set("ProductName", "Nex");
    res.compile().expect("failed to compile Windows resources");
}
