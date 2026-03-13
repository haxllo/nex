use nex_core::contract::{CoreRequest, SearchRequest};
use nex_core::core_service::CoreService;
use nex_core::hotkey_runtime::{default_hotkey_registrar, HotkeyRegistration};
use nex_core::transport::TransportResponse;

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
    let request = CoreRequest::Search(SearchRequest {
        query: "code".into(),
        limit: Some(5),
    });
    let response = nex_core::transport::handle_request(&service, request);

    match response {
        TransportResponse::Ok { response: _ } => {}
        other => panic!("unexpected transport response: {other:?}"),
    }
}

#[cfg(target_os = "windows")]
#[test]
fn windows_runtime_smoke_registers_hotkey_and_transport_roundtrip() {
    if std::env::var("NEX_WINDOWS_RUNTIME_SMOKE").as_deref() != Ok("1") {
        eprintln!(
            "skipping windows runtime smoke (set NEX_WINDOWS_RUNTIME_SMOKE=1 to enable)"
        );
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
    let payload = serde_json::to_string(&CoreRequest::Search(SearchRequest {
        query: "code".into(),
        limit: Some(5),
    }))
    .expect("request should serialize");

    let response = nex_core::transport::handle_json(&service, &payload);
    let parsed: TransportResponse =
        serde_json::from_str(&response).expect("response should deserialize");

    match parsed {
        TransportResponse::Ok { response: _ } => {}
        other => panic!("expected ok transport response, got {other:?}"),
    }
}
