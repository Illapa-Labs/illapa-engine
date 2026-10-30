// SPDX-License-Identifier: AGPL-3.0-or-later
//! Protection against floods of requests. The API is public and read-only, so nothing can be stolen or changed; these
//! keep an attack cheap instead:
//! - Only Cloudflare reaches the data routes. Cloudflare adds a secret header (`ORIGIN_SECRET`) to every request, so
//!   requests that go straight to Fly (`illapa-engine.fly.dev`) skip the CDN and are refused. `/healthz` stays open for
//!   Fly's health checks.
//! - Routes without filters refuse any query string: random parameters, used to dodge the CDN cache, get a tiny 400.
//! - A cap on simultaneous requests and a time limit: over the cap the API answers 503 at once instead of queueing.

use axum::{
    Json,
    extract::{Request, State},
    http::{HeaderValue, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use serde_json::json;
use std::{sync::Arc, time::Duration};

/// The header Cloudflare adds with the shared secret (a Transform Rule, see docs/DEPLOY.md).
pub const ORIGIN_HEADER: &str = "x-illapa-origin";
/// Requests handled at the same time; beyond this, new ones are refused right away.
pub const MAX_CONCURRENT_REQUESTS: usize = 128;
/// No request takes longer than this.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// A refusal that is never cached, so it does not stick in the CDN.
fn refuse(status: StatusCode, message: &str) -> Response {
    let mut response = (status, Json(json!({ "error": message }))).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// Compares without stopping at the first difference, so the response time gives no hint.
fn same_secret(given: &[u8], expected: &[u8]) -> bool {
    given.len() == expected.len() && given.iter().zip(expected).fold(0u8, |difference, (a, b)| difference | (a ^ b)) == 0
}

pub async fn require_origin_secret(State(secret): State<Arc<str>>, request: Request, next: Next) -> Response {
    let allowed = request
        .headers()
        .get(ORIGIN_HEADER)
        .is_some_and(|given| same_secret(given.as_bytes(), secret.as_bytes()));
    if allowed {
        next.run(request).await
    } else {
        refuse(StatusCode::FORBIDDEN, "acceso directo no permitido; usa https://api.illapalabs.org")
    }
}

pub async fn reject_query(request: Request, next: Next) -> Response {
    if request.uri().query().is_some_and(|query| !query.is_empty()) {
        refuse(StatusCode::BAD_REQUEST, "esta ruta no acepta parámetros")
    } else {
        next.run(request).await
    }
}

/// What the cap and the time limit answer.
pub async fn overloaded(_: tower::BoxError) -> Response {
    refuse(
        StatusCode::SERVICE_UNAVAILABLE,
        "la API está ocupada; vuelve a intentar en unos segundos",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_must_match_exactly() {
        assert!(same_secret(b"abc123", b"abc123"));
        assert!(!same_secret(b"abc124", b"abc123"));
        assert!(!same_secret(b"abc12", b"abc123"));
        assert!(!same_secret(b"", b"abc123"));
    }
}
