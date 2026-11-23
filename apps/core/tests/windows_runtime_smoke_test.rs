use nex_core::core_service::CoreService;
use nex_core::hotkey_runtime::{default_hotkey_registrar, HotkeyRegistration};

fn seed_service() -> CoreService {
    let config = nex_core::config::Config::default();
    let db = nex_core::index_store::open_memory().expect("in-memory db should open");
    let service = CoreService::with_connection(config, db).expect("service should initialize");
    service
        .upsert_item(&nex_core::model::SearchItem::new(
            "seed",
            "app",
            "Visual Studio Code",
            "C:\\Program Files\\Microsoft VS Code\\Code.exe",
        ))
        .expect("seed item should upsert");
    service
}

#[cfg(not(target_os = "windows"))]
#[test]
fn non_windows_fallback_smoke_still_roundtrips() {
    let mut registrar = default_hotkey_registrar();
    let registration = registrar
        .register_hotkey("Alt+Space")
        .expect("non-windows registrar should return noop registration");
    assert_eq!(
        registration,
        HotkeyRegistration::Noop("Alt+Space".to_string())
    );
    registrar
        .unregister_all()
        .expect("non-windows registrar should unregister noop entries");

    let service = seed_service();
    let results = service.search("code", 5).expect("search should answer");
    assert!(
        !results.is_empty(),
        "the seeded item should be found through the live search path"
    );
}

#[cfg(target_os = "windows")]
#[test]
fn windows_runtime_smoke_registers_hotkey_and_searches() {
    if std::env::var("NEX_WINDOWS_RUNTIME_SMOKE").as_deref() != Ok("1") {
        eprintln!("skipping windows runtime smoke (set NEX_WINDOWS_RUNTIME_SMOKE=1 to enable)");
        return;
    }

    let mut registrar = default_hotkey_registrar();
    let candidates = ["Ctrl+Shift+F12", "Ctrl+Shift+F11", "Alt+F10"];

    let mut registration = None;
    for candidate in candidates {
        match registrar.register_hotkey(candidate) {
            Ok(registered) => {
                registration = Some(registered);
                break;
            }
            Err(_) => continue,
        }
    }

    let registered = registration.expect("expected at least one hotkey registration to succeed");
    match registered {
        HotkeyRegistration::Native(_) => {}
        other => panic!("expected native registration on windows, got {other:?}"),
    }

    registrar
        .unregister_all()
        .expect("unregister should succeed after registration");

    let service = seed_service();
    let results = service.search("code", 5).expect("search should answer");
    assert!(
        !results.is_empty(),
        "the seeded item should be found through the live search path"
    );
}
