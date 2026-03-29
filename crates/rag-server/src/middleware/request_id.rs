use axum::http::{HeaderValue, Request};
use axum::middleware::Next;
use axum::response::Response;
use uuid::Uuid;

/// Middleware that ensures every request has a UUID request ID.
///
/// Reads `x-request-id` from the incoming request. If missing or not a valid
/// UUID, generates a new v4 UUID. The ID is inserted into request extensions
/// and echoed on the response.
pub async fn request_id(mut req: Request<axum::body::Body>, next: Next) -> Response {
    let id = req
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<Uuid>().ok())
        .unwrap_or_else(Uuid::new_v4);

    req.extensions_mut().insert(id);

    let mut response = next.run(req).await;

    if let Ok(val) = HeaderValue::from_str(&id.to_string()) {
        response.headers_mut().insert("x-request-id", val);
    }

    response
}
