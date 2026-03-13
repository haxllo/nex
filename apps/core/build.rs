fn main() {
    println!("cargo:rerun-if-changed=../assets/nex.ico");
    println!("cargo:rerun-if-changed=../assets/nex.png");

    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "debug".to_string());
    let allow_missing_icon = std::env::var("NEX_ALLOW_MISSING_ICON")
        .or_else(|_| std::env::var("SWIFTFIND_ALLOW_MISSING_ICON"))
        .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "windows" {
        return;
    }

    let icon_path = "../assets/nex.ico";
    if !std::path::Path::new(icon_path).exists() {
        if profile == "release" && !allow_missing_icon {
            panic!(
                "missing Windows icon file for release build: {icon_path}. Add apps/assets/nex.ico"
            );
        }
        println!(
            "cargo:warning=nex: Windows icon missing at {icon_path}; continuing without embedded icon"
        );
        return;
    }

    let mut res = winres::WindowsResource::new();
    res.set_icon(icon_path);
    res.compile().expect("failed to compile Windows resources");
}
