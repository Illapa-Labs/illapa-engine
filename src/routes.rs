// SPDX-License-Identifier: AGPL-3.0-or-later
//! The HTTP endpoints. Field names are in Spanish, like the data they describe (ubigeo, aviso, distrito).
//! Every list is served from memory (see `snapshot.rs`): requests never query the database or SENAMHI.

use axum::{
    Json,
    extract::{Query, State},
    http::{HeaderName, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};

use crate::{AppState, snapshot::Filters};

const SOURCE: &str = "Avisos: SENAMHI. Población: INEI, Censo 2017. No es un sistema oficial de alerta.";

/// How long each kind of data can be kept, given how often it changes. Browsers read `Cache-Control`; CDNs read
/// `CDN-Cache-Control` (RFC 9213), which Cloudflare prefers over `Cache-Control`. There is no `s-maxage`: it would
/// stop a CDN from serving a stale copy while it fetches a new one.
#[derive(Clone, Copy)]
pub enum Freshness {
    /// Warnings and district lists, refreshed every 30 minutes: browsers keep them 5 minutes and CDNs 10, and both
    /// may serve the previous copy while they fetch the new one, so the data is never more than about half an hour
    /// older than the last refresh.
    Warnings,
    /// The forecast, refreshed every 6 hours: half an hour everywhere.
    Forecast,
}

/// If the API fails, a CDN may keep serving the last copy for a day: an old warning is better than an empty page.
const STALE_IF_ERROR: &str = "stale-if-error=86400";

impl Freshness {
    fn browser(self) -> String {
        match self {
            Self::Warnings => format!("public, max-age=300, stale-while-revalidate=1500, {STALE_IF_ERROR}"),
            Self::Forecast => format!("public, max-age=1800, stale-while-revalidate=3600, {STALE_IF_ERROR}"),
        }
    }

    fn cdn(self) -> String {
        match self {
            Self::Warnings => format!("max-age=600, stale-while-revalidate=1200, {STALE_IF_ERROR}"),
            Self::Forecast => format!("max-age=1800, stale-while-revalidate=3600, {STALE_IF_ERROR}"),
        }
    }
}

const CDN_CACHE_CONTROL: HeaderName = HeaderName::from_static("cdn-cache-control");

fn cached(body: Value, freshness: Freshness) -> Response {
    let mut response = Json(body).into_response();
    let headers = response.headers_mut();
    for (name, value) in [(header::CACHE_CONTROL, freshness.browser()), (CDN_CACHE_CONTROL, freshness.cdn())] {
        headers.insert(name, HeaderValue::from_str(&value).expect("cache header values are ASCII"));
    }
    response
}

/// Not ready yet: never cached, so the next request tries again.
fn unavailable(message: &str) -> Response {
    let mut response = (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "error": message }))).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

pub async fn health() -> &'static str {
    "ok"
}

/// The current rain warning as GeoJSON, ready for the map.
pub async fn aviso(State(state): State<AppState>) -> Response {
    match state.aviso.read().await.as_ref() {
        Some(aviso) => cached(aviso.clone(), Freshness::Warnings),
        None => unavailable("todavía no hay un aviso cargado; vuelve a intentar en unos minutos"),
    }
}

/// The hourly rain forecast on the grid over Peru (see `forecast.rs` for the format).
pub async fn pronostico(State(state): State<AppState>) -> Response {
    match state.pronostico.read().await.as_ref() {
        Some(forecast) => cached(forecast.clone(), Freshness::Forecast),
        None => unavailable("todavía no hay un pronóstico cargado; vuelve a intentar en unos minutos"),
    }
}

/// A list with its source and how fresh the data is. Before the first refresh there are no warnings yet, which is
/// not the same as "no warnings": the answer is 503 and is not cached.
fn list(key: &str, items: Value, total: usize, snapshot: &crate::snapshot::Snapshot) -> Response {
    if snapshot.actualizado.is_none() {
        return unavailable("todavía no hay datos del SENAMHI; vuelve a intentar en unos minutos");
    }
    cached(
        json!({
            "fuente": SOURCE,
            "total": total,
            key: items,
            "actualizado": snapshot.actualizado,
            "fecha_quebradas": snapshot.fecha_quebradas,
        }),
        Freshness::Warnings,
    )
}

pub async fn departamentos(State(state): State<AppState>) -> Response {
    let snapshot = state.snapshot.read().await.clone();
    list("departamentos", json!(snapshot.departments), snapshot.departments.len(), &snapshot)
}

pub async fn provincias(State(state): State<AppState>, Query(filters): Query<Filters>) -> Response {
    let snapshot = state.snapshot.read().await.clone();
    let items = snapshot.provinces(&filters);
    list("provincias", json!(items), items.len(), &snapshot)
}

pub async fn distritos(State(state): State<AppState>, Query(filters): Query<Filters>) -> Response {
    let snapshot = state.snapshot.read().await.clone();
    let items = snapshot.districts(&filters);
    list("distritos", json!(items), items.len(), &snapshot)
}
