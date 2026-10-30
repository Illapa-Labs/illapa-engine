// SPDX-License-Identifier: AGPL-3.0-or-later
//! Hourly rain forecast for the next hours, on a fixed grid of 0.2° (about 22 km) over Peru
//! (`data/malla-pronostico.json`) and, where there are ravines, on finer grids of 0.1° (`data/zonas-pronostico.json`),
//! the real resolution of the model. It comes from the UK Met Office global 10 km model, served by Open-Meteo (its free
//! API is for non-commercial use). The Met Office data is CC BY-SA 4.0: it must be credited, and what is made from it
//! (this forecast and the maps drawn from it) must be shared under the same licence.
//!
//! Of the models Open-Meteo offers, it was the one that best caught the heavy-rain days over the ravines of Chosica
//! and Chaclacayo in the 2022-23 to 2024-25 rainy seasons, compared with SENAMHI's observed rainfall (PISCOp).

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::PgPool;
use std::time::Duration;

/// Hours from the current one: it is refreshed every 12 hours, so 36 always cover the next 24. More hours do not
/// cost more calls.
const HOURS: usize = 36;
/// Open-Meteo allows 600 calls a minute, 5 000 an hour and 10 000 a day. It is not clear whether a request with
/// many points counts as one call or one per point, so points are asked in small batches with pauses, and the budget
/// counts every point: about 3 870 per run (the national grid plus the 0.1° zones), two runs a day, which leaves
/// room for the retries after a failure. A test keeps the total under `DAILY_POINTS_BUDGET`.
const POINTS_PER_REQUEST: usize = 290;
/// Runs a day (see `FORECAST_REFRESH_SECS`, 12 hours) times the points of each run must stay under this, a bit below
/// Open-Meteo's 10 000 a day.
#[cfg(test)]
const DAILY_POINTS_BUDGET: usize = 8_200;
const PAUSE: Duration = Duration::from_secs(40);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
/// The Open-Meteo model. Without one, Open-Meteo picks ECMWF IFS for Peru.
const MODEL: &str = "ukmo_global_deterministic_10km";
const SOURCE: &str = "Pronóstico: UK Met Office (Crown copyright), vía Open-Meteo";
const LICENSE: &str = "CC BY-SA 4.0";

/// A grid: its north-west corner, step in degrees, size, and the cells (row * cols + col) worth asking for.
#[derive(Deserialize)]
struct Grid {
    west: f64,
    north: f64,
    step: f64,
    cols: usize,
    rows: usize,
    cells: Vec<usize>,
}

/// A finer grid over an area with ravines.
#[derive(Deserialize)]
struct Zone {
    id: String,
    nombre: String,
    #[serde(flatten)]
    grid: Grid,
}

#[derive(Deserialize)]
struct Zones {
    zonas: Vec<Zone>,
}

fn grid() -> Result<Grid> {
    serde_json::from_str(include_str!("../data/malla-pronostico.json")).context("forecast grid")
}

fn zones() -> Result<Vec<Zone>> {
    Ok(serde_json::from_str::<Zones>(include_str!("../data/zonas-pronostico.json"))
        .context("forecast zones")?
        .zonas)
}

/// Whether a saved forecast was made on the current grid, zones and model (a new grid, zone or model needs a new
/// forecast right away).
pub fn on_current_grid(forecast: &Value) -> bool {
    let same_zones = zones().is_ok_and(|zones| {
        let saved = forecast["zonas"].as_array().map(Vec::as_slice).unwrap_or_default();
        saved.len() == zones.len()
            && saved.iter().zip(&zones).all(|(saved, zone)| {
                saved["id"].as_str() == Some(zone.id.as_str())
                    && saved["paso"].as_f64() == Some(zone.grid.step)
                    && saved["columnas"].as_u64() == Some(zone.grid.cols as u64)
                    && saved["filas"].as_u64() == Some(zone.grid.rows as u64)
            })
    });
    same_zones
        && grid().is_ok_and(|grid| {
            forecast["paso"].as_f64() == Some(grid.step)
                && forecast["horas"].as_u64() == Some(HOURS as u64)
                && forecast["modelo"].as_str() == Some(MODEL)
        })
}

/// Asks Open-Meteo for every cell of a grid, in batches with pauses between them (`first` is true until the very
/// first batch of the run, which is the only one that does not wait). Values are tenths of mm of rain in the previous
/// hour, per hour, row after row; -1 where nothing was asked (outside Peru) or Open-Meteo has no value.
async fn fetch_grid(
    http: &reqwest::Client,
    open_meteo_url: &str,
    grid: &Grid,
    label: &str,
    first: &mut bool,
    start: &mut Option<String>,
) -> Result<Vec<Vec<i32>>> {
    let mut values = vec![vec![-1i32; grid.cols * grid.rows]; HOURS];

    for (index, batch) in grid.cells.chunks(POINTS_PER_REQUEST).enumerate() {
        if !std::mem::replace(first, false) {
            tokio::time::sleep(PAUSE).await;
        }
        let latitude = |cell: &usize| format!("{:.2}", grid.north - (cell / grid.cols) as f64 * grid.step);
        let longitude = |cell: &usize| format!("{:.2}", grid.west + (cell % grid.cols) as f64 * grid.step);
        let latitudes: Vec<String> = batch.iter().map(latitude).collect();
        let longitudes: Vec<String> = batch.iter().map(longitude).collect();
        let url = format!(
            "{open_meteo_url}?latitude={}&longitude={}&hourly=precipitation&forecast_hours={HOURS}&timezone=GMT&models={MODEL}",
            latitudes.join(","),
            longitudes.join(",")
        );
        let response = http.get(url).timeout(REQUEST_TIMEOUT).send().await.context("Open-Meteo")?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            bail!("Open-Meteo {status}: {}", body.chars().take(200).collect::<String>());
        }
        let body: Value = response.json().await.context("Open-Meteo")?;
        let results: Vec<&Value> = match &body {
            Value::Array(results) => results.iter().collect(),
            single => vec![single],
        };
        if results.len() != batch.len() {
            bail!("Open-Meteo returned {} points for {} asked", results.len(), batch.len());
        }
        for (cell, result) in batch.iter().zip(results) {
            if start.is_none() {
                *start = result["hourly"]["time"][0].as_str().map(|time| format!("{time}:00Z"));
            }
            let hourly = result["hourly"]["precipitation"].as_array().into_iter().flatten();
            for (hour, mm) in hourly.enumerate().take(HOURS) {
                if let Some(mm) = mm.as_f64() {
                    values[hour][*cell] = (mm * 10.0).round() as i32;
                }
            }
        }
        tracing::info!(
            grid = label,
            points = ((index + 1) * POINTS_PER_REQUEST).min(grid.cells.len()),
            of = grid.cells.len(),
            "forecast batch"
        );
    }
    Ok(values)
}

/// Asks Open-Meteo for the national grid and then for each 0.1° zone.
pub async fn fetch(http: &reqwest::Client, open_meteo_url: &str) -> Result<Value> {
    let grid = grid()?;
    let zones = zones()?;
    let mut start: Option<String> = None;
    let mut first = true;

    let values = fetch_grid(http, open_meteo_url, &grid, "peru", &mut first, &mut start).await?;
    let mut fine = Vec::with_capacity(zones.len());
    for zone in &zones {
        let values = fetch_grid(http, open_meteo_url, &zone.grid, &zone.id, &mut first, &mut start).await?;
        fine.push(json!({
            "id": zone.id,
            "nombre": zone.nombre,
            "oeste": zone.grid.west,
            "norte": zone.grid.north,
            "paso": zone.grid.step,
            "columnas": zone.grid.cols,
            "filas": zone.grid.rows,
            "valores": values,
        }));
    }

    Ok(json!({
        "generado": chrono::Utc::now().to_rfc3339(),
        "inicio": start.context("Open-Meteo returned no hours")?,
        "horas": HOURS,
        "oeste": grid.west,
        "norte": grid.north,
        "paso": grid.step,
        "columnas": grid.cols,
        "filas": grid.rows,
        "valores": values,
        "zonas": fine,
        "fuente": SOURCE,
        "licencia": LICENSE,
        "modelo": MODEL,
    }))
}

pub async fn save(pool: &PgPool, forecast: &Value) -> Result<()> {
    sqlx::query(
        "insert into pronostico (id, datos, actualizado) values (1, $1, now())
         on conflict (id) do update set datos = excluded.datos, actualizado = excluded.actualizado",
    )
    .bind(sqlx::types::Json(forecast))
    .execute(pool)
    .await
    .context("saving the forecast")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_grid_covers_peru_with_valid_cells() {
        let grid = grid().unwrap();
        assert!(!grid.cells.is_empty());
        assert!(grid.cells.iter().all(|cell| *cell < grid.cols * grid.rows));
    }

    #[test]
    fn the_zones_are_finer_than_the_national_grid_and_have_valid_cells() {
        let national = grid().unwrap();
        let zones = zones().unwrap();
        assert!(!zones.is_empty());
        let mut ids: Vec<&str> = zones.iter().map(|zone| zone.id.as_str()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), zones.len(), "zone ids must be unique");
        for zone in &zones {
            assert!(zone.grid.step < national.step, "{} is not finer", zone.id);
            assert!(!zone.grid.cells.is_empty());
            assert!(
                zone.grid.cells.iter().all(|cell| *cell < zone.grid.cols * zone.grid.rows),
                "{}",
                zone.id
            );
        }
    }

    #[test]
    fn a_run_fits_the_daily_budget_of_open_meteo() {
        let points = grid().unwrap().cells.len() + zones().unwrap().iter().map(|zone| zone.grid.cells.len()).sum::<usize>();
        assert!(points * 2 <= DAILY_POINTS_BUDGET, "{points} points a run, two runs a day");
    }

    fn current_forecast() -> Value {
        let grid = grid().unwrap();
        let saved_zones: Vec<Value> = zones()
            .unwrap()
            .iter()
            .map(|zone| json!({ "id": zone.id, "paso": zone.grid.step, "columnas": zone.grid.cols, "filas": zone.grid.rows }))
            .collect();
        json!({ "paso": grid.step, "horas": HOURS, "modelo": MODEL, "zonas": saved_zones })
    }

    #[test]
    fn a_forecast_on_another_grid_zones_or_model_is_not_reused() {
        assert!(on_current_grid(&current_forecast()));
        // Another national grid or length
        let mut other = current_forecast();
        other["paso"] = json!(0.3);
        assert!(!on_current_grid(&other));
        let mut other = current_forecast();
        other["horas"] = json!(30);
        assert!(!on_current_grid(&other));
        // Before the zones existed, or with a different set of them
        let mut without = current_forecast();
        without.as_object_mut().unwrap().remove("zonas");
        assert!(!on_current_grid(&without));
        let mut other = current_forecast();
        other["zonas"][0]["paso"] = json!(0.2);
        assert!(!on_current_grid(&other));
        // Another model, or none saved
        let mut other = current_forecast();
        other["modelo"] = json!("ecmwf_ifs");
        assert!(!on_current_grid(&other));
        assert!(!on_current_grid(&json!({})));
    }
}
