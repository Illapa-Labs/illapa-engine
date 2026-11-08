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
    /// The forecast, refreshed every 12 hours: half an hour everywhere.
    Forecast,
    /// The list of cloud images, with a new one every 10 minutes: 5 minutes everywhere.
    Clouds,
    /// The river flood warnings, published once a day and checked every hour: half an hour everywhere, and an hour
    /// more serving the previous copy.
    Floods,
}

/// If the API fails, a CDN may keep serving the last copy for a day: an old warning is better than an empty page.
const STALE_IF_ERROR: &str = "stale-if-error=86400";

impl Freshness {
    fn browser(self) -> String {
        match self {
            Self::Warnings => format!("public, max-age=300, stale-while-revalidate=1500, {STALE_IF_ERROR}"),
            Self::Forecast => format!("public, max-age=1800, stale-while-revalidate=3600, {STALE_IF_ERROR}"),
            Self::Clouds => format!("public, max-age=300, stale-while-revalidate=600, {STALE_IF_ERROR}"),
            Self::Floods => format!("public, max-age=1800, stale-while-revalidate=3600, {STALE_IF_ERROR}"),
        }
    }

    fn cdn(self) -> String {
        match self {
            Self::Warnings => format!("max-age=600, stale-while-revalidate=1200, {STALE_IF_ERROR}"),
            Self::Forecast => format!("max-age=1800, stale-while-revalidate=3600, {STALE_IF_ERROR}"),
            Self::Clouds => format!("max-age=300, stale-while-revalidate=600, {STALE_IF_ERROR}"),
            Self::Floods => format!("max-age=1800, stale-while-revalidate=3600, {STALE_IF_ERROR}"),
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

/// Not there: never cached.
fn not_found(message: &str) -> Response {
    let mut response = (StatusCode::NOT_FOUND, Json(json!({ "error": message }))).into_response();
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

/// The GOES-19 cloud images of the last 12 hours (one every 30 minutes, plus the newest), oldest first. Each image
/// covers `oeste`, `sur`, `este`, `norte` with its rows in Mercator, like the forecast images of the website.
pub async fn nubes(State(state): State<AppState>) -> Response {
    let frames = state.nubes.read().await;
    if frames.is_empty() {
        return unavailable("todavía no hay imágenes del satélite; vuelve a intentar en unos minutos");
    }
    let [oeste, sur, este, norte] = crate::clouds::BOUNDS;
    let imagenes: Vec<Value> = frames
        .keys()
        .map(|time| json!({ "hora": time.to_rfc3339_opts(chrono::SecondsFormat::Secs, true), "url": format!("/v1/nubes/{}.png", time.format("%Y%m%dT%H%M%S")) }))
        .collect();
    cached(
        json!({
            "fuente": "Nubes: satélite GOES-19 (NOAA), banda 13, infrarrojo de 10,3 µm",
            "licencia": "Dominio público",
            "oeste": oeste, "sur": sur, "este": este, "norte": norte,
            "imagenes": imagenes,
        }),
        Freshness::Clouds,
    )
}

/// One cloud image. It never changes, so it can be kept for a day (it is dropped from the list after 12 hours).
pub async fn nube(State(state): State<AppState>, axum::extract::Path(archivo): axum::extract::Path<String>) -> Response {
    let time = archivo
        .strip_suffix(".png")
        .and_then(|stamp| chrono::NaiveDateTime::parse_from_str(stamp, "%Y%m%dT%H%M%S").ok())
        .map(|time| time.and_utc());
    let png = match time {
        Some(time) => state.nubes.read().await.get(&time).cloned(),
        None => None,
    };
    let Some(png) = png else {
        return not_found("no hay una imagen con ese nombre");
    };
    let mut response = (StatusCode::OK, png.as_ref().clone()).into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("image/png"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=86400, immutable"));
    headers.insert(CDN_CACHE_CONTROL, HeaderValue::from_static("max-age=86400, immutable"));
    response
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

/// SENAMHI's river flood warnings (SONICS-PBI): what each level means, each day's date and totals, and what each
/// district has in the warning areas (see `floods.rs` for the format).
pub async fn inundaciones(State(state): State<AppState>) -> Response {
    match state.inundaciones.read().await.as_ref() {
        Some(floods) => cached(floods.resumen.clone(), Freshness::Floods),
        None => unavailable("todavía no hay avisos de inundación cargados; vuelve a intentar en unos minutos"),
    }
}

/// The warning areas of one day as GeoJSON, ready for the map: 0 is the current state, 1 to 7 the forecast.
pub async fn inundacion(State(state): State<AppState>, axum::extract::Path(dia): axum::extract::Path<String>) -> Response {
    let Some(day) = dia.parse::<usize>().ok().filter(|day| *day < crate::floods::DAYS) else {
        return not_found("el día va de 0 (estado actual) a 7");
    };
    match state.inundaciones.read().await.as_ref() {
        Some(floods) => cached(floods.dias[day].clone(), Freshness::Floods),
        None => unavailable("todavía no hay avisos de inundación cargados; vuelve a intentar en unos minutos"),
    }
}
