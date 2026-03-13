fn main() {
    let stdio_enabled = std::env::var("NEX_SUPPRESS_STDIO")
        .or_else(|_| std::env::var("SWIFTFIND_SUPPRESS_STDIO"))
        .map(|value| !(value == "1" || value.eq_ignore_ascii_case("true")))
        .unwrap_or(true);

    let args: Vec<String> = std::env::args().skip(1).collect();
    let options = match nex_core::runtime::parse_cli_args(&args) {
        Ok(options) => options,
        Err(error) => {
            if stdio_enabled {
                eprintln!("[nex] {error}");
            }
            std::process::exit(2);
        }
    };

    if let Err(error) = nex_core::runtime::run_with_options(options) {
        if stdio_enabled {
            eprintln!("[nex] runtime failed: {error}");
        }
        std::process::exit(1);
    }
}
