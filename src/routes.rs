// SPDX-License-Identifier: AGPL-3.0-or-later
//! The HTTP endpoints. Field names are in Spanish, like the data they describe (ubigeo, aviso, distrito).

use axum::{
    Json,
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::Row;

use crate::AppState;

const SOURCE: &str = "Avisos: SENAMHI. Población: INEI, Censo 2017. No es un sistema oficial de alerta.";

/// Errors are logged in full and answered with a short message, without internal details.
pub struct ApiError(anyhow::Error);

impl<E: Into<anyhow::Error>> From<E> for ApiError {
    fn from(error: E) -> Self {
        Self(error.into())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        tracing::error!("request failed: {:#}", self.0);
        (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "error interno" }))).into_response()
    }
}

type ApiResult = Result<Json<Value>, ApiError>;

/// SENAMHI levels and their color names.
fn color(level: i32) -> Option<&'static str> {
    match level {
        2 => Some("amarillo"),
        3 => Some("naranja"),
        4 => Some("rojo"),
        _ => None,
    }
}

fn levels(rain: i32, ravine: i32) -> Value {
    let level = rain.max(ravine);
    json!({ "nivel": level, "color": color(level), "nivel_lluvia": rain, "nivel_quebradas": ravine })
}

fn with(mut item: Value, extra: Value) -> Value {
    if let (Some(item), Value::Object(extra)) = (item.as_object_mut(), extra) {
        item.extend(extra);
    }
    item
}

/// When the data was refreshed and the date of the ravine warning, for every list.
async fn freshness(state: &AppState) -> Result<Value, ApiError> {
    let row = sqlx::query("select actualizado, fecha_quebradas from refresco where id = 1")
        .fetch_optional(&state.pool)
        .await?;
    Ok(match row {
        Some(row) => json!({
            "actualizado": row.get::<chrono::DateTime<chrono::Utc>, _>("actualizado").to_rfc3339(),
            "fecha_quebradas": row.get::<Option<String>, _>("fecha_quebradas"),
        }),
        None => json!({ "actualizado": null, "fecha_quebradas": null }),
    })
}

pub async fn health() -> &'static str {
    "ok"
}

/// The current rain warning as GeoJSON, ready for the map.
pub async fn aviso(State(state): State<AppState>) -> Response {
    match state.aviso.read().await.as_ref() {
        Some(aviso) => Json(aviso.clone()).into_response(),
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "todavía no hay un aviso cargado; vuelve a intentar en unos minutos" })),
        )
            .into_response(),
    }
}

/// The hourly rain forecast on the grid over Peru (see `forecast.rs` for the format).
pub async fn pronostico(State(state): State<AppState>) -> Response {
    match state.pronostico.read().await.as_ref() {
        Some(forecast) => Json(forecast.clone()).into_response(),
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "todavía no hay un pronóstico cargado; vuelve a intentar en unos minutos" })),
        )
            .into_response(),
    }
}

#[derive(Deserialize, Default)]
pub struct Filters {
    /// Department ubigeo (2 digits) or name, for example `15` or `lima`
    departamento: Option<String>,
    /// Province ubigeo (4 digits) or name
    provincia: Option<String>,
    /// Only those with a warning of this level or higher (2, 3 or 4)
    nivel_minimo: Option<i32>,
    /// `true`: only those with some warning
    solo_avisos: Option<bool>,
    /// Maximum number of rows
    limite: Option<i64>,
}

impl Filters {
    fn minimum_level(&self) -> i32 {
        let minimum = self.nivel_minimo.unwrap_or(0);
        if self.solo_avisos.unwrap_or(false) {
            minimum.max(2)
        } else {
            minimum
        }
    }
}

pub async fn departamentos(State(state): State<AppState>) -> ApiResult {
    let rows = sqlx::query(
        "select d.ubigeo, d.nombre, d.poblacion_censada, d.poblacion_total,
                coalesce(max(greatest(a.nivel_lluvia, a.nivel_quebradas)), 0)::int as nivel,
                count(a.ubigeo) filter (where greatest(a.nivel_lluvia, a.nivel_quebradas) >= 2)::int as distritos_con_aviso,
                coalesce(sum(a.personas_amarillo + a.personas_naranja + a.personas_rojo), 0)::bigint as personas_en_zona
           from departamentos d
           left join distritos x on x.departamento_ubigeo = d.ubigeo
           left join distrito_aviso a on a.ubigeo = x.ubigeo
          group by d.ubigeo
          order by nivel desc, distritos_con_aviso desc, d.nombre",
    )
    .fetch_all(&state.pool)
    .await?;

    let items: Vec<Value> = rows
        .iter()
        .map(|row| {
            let level: i32 = row.get("nivel");
            json!({
                "ubigeo": row.get::<String, _>("ubigeo").trim(),
                "nombre": row.get::<String, _>("nombre"),
                "poblacion": row.get::<i32, _>("poblacion_censada"),
                "poblacion_total": row.get::<i32, _>("poblacion_total"),
                "nivel": level,
                "color": color(level),
                "distritos_con_aviso": row.get::<i32, _>("distritos_con_aviso"),
                "personas_en_zona": row.get::<i64, _>("personas_en_zona"),
            })
        })
        .collect();
    Ok(Json(with(
        json!({ "fuente": SOURCE, "total": items.len(), "departamentos": items }),
        freshness(&state).await?,
    )))
}

pub async fn provincias(State(state): State<AppState>, Query(filters): Query<Filters>) -> ApiResult {
    let rows = sqlx::query(
        "select p.ubigeo, p.nombre, p.poblacion_censada, p.poblacion_total, dep.ubigeo as dep_ubigeo, dep.nombre as dep,
                coalesce(max(a.nivel_lluvia), 0)::int as nivel_lluvia,
                coalesce(max(a.nivel_quebradas), 0)::int as nivel_quebradas,
                count(a.ubigeo) filter (where greatest(a.nivel_lluvia, a.nivel_quebradas) >= 2)::int as distritos_con_aviso,
                coalesce(sum(a.personas_amarillo + a.personas_naranja + a.personas_rojo), 0)::bigint as personas_en_zona
           from provincias p
           join departamentos dep on dep.ubigeo = p.departamento_ubigeo
           left join distritos x on x.provincia_ubigeo = p.ubigeo
           left join distrito_aviso a on a.ubigeo = x.ubigeo
          where ($1::text is null or dep.ubigeo = $1 or upper(dep.nombre) = upper($1))
            and ($2::text is null or p.ubigeo = $2 or upper(p.nombre) = upper($2))
          group by p.ubigeo, dep.ubigeo, dep.nombre
         having greatest(coalesce(max(a.nivel_lluvia), 0), coalesce(max(a.nivel_quebradas), 0)) >= $3
          order by greatest(coalesce(max(a.nivel_lluvia), 0), coalesce(max(a.nivel_quebradas), 0)) desc, p.nombre
          limit $4",
    )
    .bind(filters.departamento.as_deref())
    .bind(filters.provincia.as_deref())
    .bind(filters.minimum_level())
    .bind(filters.limite.unwrap_or(500).clamp(1, 500))
    .fetch_all(&state.pool)
    .await?;

    let items: Vec<Value> = rows
        .iter()
        .map(|row| {
            let item = json!({
                "ubigeo": row.get::<String, _>("ubigeo").trim(),
                "nombre": row.get::<String, _>("nombre"),
                "departamento": { "ubigeo": row.get::<String, _>("dep_ubigeo").trim(), "nombre": row.get::<String, _>("dep") },
                "poblacion": row.get::<i32, _>("poblacion_censada"),
                "poblacion_total": row.get::<i32, _>("poblacion_total"),
                "distritos_con_aviso": row.get::<i32, _>("distritos_con_aviso"),
                "personas_en_zona": row.get::<i64, _>("personas_en_zona"),
            });
            with(item, levels(row.get("nivel_lluvia"), row.get("nivel_quebradas")))
        })
        .collect();
    Ok(Json(with(
        json!({ "fuente": SOURCE, "total": items.len(), "provincias": items }),
        freshness(&state).await?,
    )))
}

pub async fn distritos(State(state): State<AppState>, Query(filters): Query<Filters>) -> ApiResult {
    let rows = sqlx::query(
        "select x.ubigeo, x.nombre, x.poblacion_censada, x.poblacion_total,
                p.ubigeo as prov_ubigeo, p.nombre as prov, dep.ubigeo as dep_ubigeo, dep.nombre as dep,
                coalesce(a.nivel_lluvia, 0)::int as nivel_lluvia, coalesce(a.nivel_quebradas, 0)::int as nivel_quebradas,
                coalesce(a.centros_poblados_amarillo, 0) as centros_amarillo,
                coalesce(a.centros_poblados_naranja, 0) as centros_naranja,
                coalesce(a.centros_poblados_rojo, 0) as centros_rojo,
                coalesce(a.personas_amarillo, 0) as personas_amarillo,
                coalesce(a.personas_naranja, 0) as personas_naranja,
                coalesce(a.personas_rojo, 0) as personas_rojo
           from distritos x
           join provincias p on p.ubigeo = x.provincia_ubigeo
           join departamentos dep on dep.ubigeo = x.departamento_ubigeo
           left join distrito_aviso a on a.ubigeo = x.ubigeo
          where ($1::text is null or dep.ubigeo = $1 or upper(dep.nombre) = upper($1))
            and ($2::text is null or p.ubigeo = $2 or upper(p.nombre) = upper($2))
            and greatest(coalesce(a.nivel_lluvia, 0), coalesce(a.nivel_quebradas, 0)) >= $3
          order by greatest(coalesce(a.nivel_lluvia, 0), coalesce(a.nivel_quebradas, 0)) desc, x.poblacion_censada desc
          limit $4",
    )
    .bind(filters.departamento.as_deref())
    .bind(filters.provincia.as_deref())
    .bind(filters.minimum_level())
    .bind(filters.limite.unwrap_or(2000).clamp(1, 2000))
    .fetch_all(&state.pool)
    .await?;

    let items: Vec<Value> = rows
        .iter()
        .map(|row| {
            let by_level = |prefix: &str| {
                json!({
                    "amarillo": row.get::<i32, _>(format!("{prefix}_amarillo").as_str()),
                    "naranja": row.get::<i32, _>(format!("{prefix}_naranja").as_str()),
                    "rojo": row.get::<i32, _>(format!("{prefix}_rojo").as_str()),
                })
            };
            let item = json!({
                "ubigeo": row.get::<String, _>("ubigeo").trim(),
                "nombre": row.get::<String, _>("nombre"),
                "provincia": { "ubigeo": row.get::<String, _>("prov_ubigeo").trim(), "nombre": row.get::<String, _>("prov") },
                "departamento": { "ubigeo": row.get::<String, _>("dep_ubigeo").trim(), "nombre": row.get::<String, _>("dep") },
                "poblacion": row.get::<i32, _>("poblacion_censada"),
                "poblacion_total": row.get::<i32, _>("poblacion_total"),
                "centros_poblados_en_zona": by_level("centros"),
                "personas_en_zona": by_level("personas"),
            });
            with(item, levels(row.get("nivel_lluvia"), row.get("nivel_quebradas")))
        })
        .collect();
    Ok(Json(with(
        json!({ "fuente": SOURCE, "total": items.len(), "distritos": items }),
        freshness(&state).await?,
    )))
}
