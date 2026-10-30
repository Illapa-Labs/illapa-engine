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

/// How the origin secret is checked.
#[derive(Clone)]
pub struct OriginCheck {
    pub secret: Arc<str>,
    /// `false`: only log what would be refused, without refusing (`ORIGIN_CHECK=report`), to try a new Cloudflare
    /// rule without taking the site down
    pub enforce: bool,
}

pub async fn require_origin_secret(State(check): State<OriginCheck>, request: Request, next: Next) -> Response {
    let given = request.headers().get(ORIGIN_HEADER).map(|value| value.as_bytes().to_vec());
    if given.as_deref().is_some_and(|given| same_secret(given, check.secret.as_bytes())) {
        return next.run(request).await;
    }
    // Only the length is logged, never the value
    let reason = match &given {
        None => "header missing".to_string(),
        Some(given) => format!(
            "header has the wrong value ({} bytes, expected {})",
            given.len(),
            check.secret.len()
        ),
    };
    if check.enforce {
        tracing::debug!(path = %request.uri().path(), "refused: {reason}");
        refuse(StatusCode::FORBIDDEN, "acceso directo no permitido; usa https://api.illapalabs.org")
    } else {
        tracing::warn!(path = %request.uri().path(), "would refuse (report only): {reason}");
        next.run(request).await
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
