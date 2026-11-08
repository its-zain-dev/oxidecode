use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub kind: &'static str,
    pub message: String,
}

impl ApiError {
    pub fn new(status: StatusCode, kind: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            kind,
            message: message.into(),
        }
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "InvalidRequestError", message)
    }

    pub fn unauthorized() -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "UnauthorizedError", "unauthorized")
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, "NotFoundError", message)
    }

    pub fn session_not_found(id: &str) -> Self {
        Self::new(
            StatusCode::NOT_FOUND,
            "SessionNotFoundError",
            format!("session {id} not found"),
        )
    }

    pub fn permission_not_found(id: &str) -> Self {
        Self::new(
            StatusCode::NOT_FOUND,
            "PermissionNotFoundError",
            format!("permission request {id} not found"),
        )
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, "ConflictError", message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "UnknownError",
            message,
        )
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = Json(json!({
            "type": self.kind,
            "message": self.message,
        }));
        (self.status, body).into_response()
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(error: anyhow::Error) -> Self {
        tracing::warn!(error = %error, "internal server error");
        Self::internal(error.to_string())
    }
}

pub type ApiResult<T> = Result<T, ApiError>;
