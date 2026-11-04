fn main() {
    println!("cargo:rerun-if-changed=../assets/nex.ico");
    println!("cargo:rerun-if-changed=build.rs");

    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "windows" {
        return;
    }

    // This package has a single binary, so its version resource can name it
    // accurately. It lives outside the `nex` crate because a build script
    // stamps every binary in its package, which is what made Nex and
    // NexIndexerProcess show the same name in Task Manager.
    let mut res = winres::WindowsResource::new();
    if std::path::Path::new("../assets/nex.ico").exists() {
        res.set_icon("../assets/nex.ico");
    }
    res.set("FileDescription", "Nex Indexer");
    res.set("InternalName", "NexIndexerProcess");
    res.set("OriginalFilename", "NexIndexerProcess.exe");
    res.set("ProductName", "Nex");
    res.compile().expect("failed to compile Windows resources");
}
