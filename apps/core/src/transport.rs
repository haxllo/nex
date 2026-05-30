use serde::{Deserialize, Serialize};

use crate::contract::{CoreRequest, CoreResponse};
use crate::core_service::{CoreService, ServiceError};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidJson,
    InvalidRequest,
    ItemNotFound,
    Launch,
    Config,
    Provider,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ErrorResponse {
    pub code: ErrorCode,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum TransportResponse {
    Ok { response: CoreResponse },
    Err { error: ErrorResponse },
}

pub fn handle_request(service: &CoreService, request: CoreRequest) -> TransportResponse {
    match service.handle_command(request) {
        Ok(response) => TransportResponse::Ok { response },
        Err(error) => TransportResponse::Err {
            error: map_service_error(error),
        },
    }
}

pub fn handle_json(service: &CoreService, payload: &str) -> String {
    let response = match serde_json::from_str::<CoreRequest>(payload) {
        Ok(request) => handle_request(service, request),
        Err(error) => TransportResponse::Err {
            error: ErrorResponse {
                code: ErrorCode::InvalidJson,
                message: error.to_string(),
            },
        },
    };

    serde_json::to_string(&response).expect("transport response should serialize")
}

fn map_service_error(error: ServiceError) -> ErrorResponse {
    match error {
        ServiceError::InvalidRequest(message) => ErrorResponse {
            code: ErrorCode::InvalidRequest,
            message,
        },
        ServiceError::ItemNotFound(message) => ErrorResponse {
            code: ErrorCode::ItemNotFound,
            message,
        },
        ServiceError::Launch(message) => ErrorResponse {
            code: ErrorCode::Launch,
            message: message.to_string(),
        },
        ServiceError::Config(message) => ErrorResponse {
            code: ErrorCode::Config,
            message,
        },
        ServiceError::Provider(message) => ErrorResponse {
            code: ErrorCode::Provider,
            message: message.to_string(),
        },
    }
}
