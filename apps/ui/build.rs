fn main() {
    println!("cargo:rerun-if-changed=../assets/nex.ico");
    println!("cargo:rerun-if-changed=build.rs");

    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "windows" {
        return;
    }

    // One binary per package: a build script's resource is linked into every
    // binary of its package AND inherited by dependent packages, so sharing a
    // package with the indexer would give both the same Task Manager name.
    let mut res = winres::WindowsResource::new();
    if std::path::Path::new("../assets/nex.ico").exists() {
        res.set_icon("../assets/nex.ico");
    }
    res.set("FileDescription", "Nex");
    res.set("InternalName", "NexUIProcess");
    res.set("OriginalFilename", "NexUIProcess.exe");
    res.set("ProductName", "Nex");
    res.compile().expect("failed to compile Windows resources");
}
