use nex_core::contract::{CoreRequest, LaunchRequest, SearchRequest};
use nex_core::core_service::CoreService;
use nex_core::transport::{handle_json, handle_request, ErrorCode, TransportResponse};

fn service_with_seed_item() -> CoreService {
    let config = nex_core::config::Config::default();
    let db = nex_core::index_store::open_memory().unwrap();
    let service = CoreService::with_connection(config, db).unwrap();
    service
        .upsert_item(&nex_core::model::SearchItem::new(
            "seed",
            "app",
            "Code",
            "C:\\Code.exe",
        ))
        .unwrap();
    service
}

#[test]
fn request_handler_returns_ok_transport_response() {
    let service = service_with_seed_item();

    let response = handle_request(
        &service,
        CoreRequest::Search(SearchRequest {
            query: "code".into(),
            limit: Some(5),
        }),
    );

    match response {
        TransportResponse::Ok { response } => {
            let encoded = serde_json::to_string(&TransportResponse::Ok { response }).unwrap();
            assert!(encoded.contains("\"status\":\"ok\""));
        }
        _ => panic!("expected ok transport response"),
    }
}

#[test]
fn json_handler_returns_invalid_json_error_code() {
    let service = service_with_seed_item();

    let raw = handle_json(&service, "{not-json");
    let parsed: TransportResponse = serde_json::from_str(&raw).unwrap();

    match parsed {
        TransportResponse::Err { error } => assert_eq!(error.code, ErrorCode::InvalidJson),
        _ => panic!("expected invalid json error"),
    }
}

#[test]
fn json_handler_returns_invalid_request_error_code() {
    let service = service_with_seed_item();
    let request = CoreRequest::Launch(LaunchRequest {
        id: Some("   ".into()),
        path: None,
    });

    let raw = handle_json(&service, &serde_json::to_string(&request).unwrap());
    let parsed: TransportResponse = serde_json::from_str(&raw).unwrap();

    match parsed {
        TransportResponse::Err { error } => assert_eq!(error.code, ErrorCode::InvalidRequest),
        _ => panic!("expected invalid request error"),
    }
}

#[test]
fn json_handler_returns_item_not_found_error_code() {
    let service = service_with_seed_item();
    let request = CoreRequest::Launch(LaunchRequest {
        id: Some("missing".into()),
        path: None,
    });

    let raw = handle_json(&service, &serde_json::to_string(&request).unwrap());
    let parsed: TransportResponse = serde_json::from_str(&raw).unwrap();

    match parsed {
        TransportResponse::Err { error } => assert_eq!(error.code, ErrorCode::ItemNotFound),
        _ => panic!("expected item not found error"),
    }
}
