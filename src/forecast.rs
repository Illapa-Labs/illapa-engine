// SPDX-License-Identifier: AGPL-3.0-or-later
//! Hourly rain forecast for the next hours, from Open-Meteo (CC BY 4.0; its free API is for non-commercial use),
//! on a fixed grid of about 33 km over Peru (`data/malla-pronostico.json`).

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::PgPool;
use std::time::Duration;

/// Hours from the current one: 24 plus some margin, because it is refreshed every 6 hours.
const HOURS: usize = 30;
/// Open-Meteo allows 600 calls a minute and 10 000 a day. It is not clear whether a request with many points
/// counts as one call or one per point, so points are asked in small batches with pauses: even counting every
/// point, that is about 1 450 calls per run.
const POINTS_PER_REQUEST: usize = 290;
const PAUSE: Duration = Duration::from_secs(40);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const SOURCE: &str = "Pronóstico: Open-Meteo (CC BY 4.0)";

/// The grid: its north-west corner, step in degrees, size, and the cells (row * cols + col) near Peru.
#[derive(Deserialize)]
struct Grid {
    west: f64,
    north: f64,
    step: f64,
    cols: usize,
    rows: usize,
    cells: Vec<usize>,
}

fn grid() -> Result<Grid> {
    serde_json::from_str(include_str!("../data/malla-pronostico.json")).context("forecast grid")
}

/// Asks Open-Meteo for the whole grid. Values are tenths of mm of rain in the previous hour, per hour, row after
/// row; -1 where nothing was asked (outside Peru) or Open-Meteo has no value.
pub async fn fetch(http: &reqwest::Client, open_meteo_url: &str) -> Result<Value> {
    let grid = grid()?;
    let mut values = vec![vec![-1i32; grid.cols * grid.rows]; HOURS];
    let mut start: Option<String> = None;

    for (index, batch) in grid.cells.chunks(POINTS_PER_REQUEST).enumerate() {
        if index > 0 {
            tokio::time::sleep(PAUSE).await;
        }
        let latitude = |cell: &usize| format!("{:.2}", grid.north - (cell / grid.cols) as f64 * grid.step);
        let longitude = |cell: &usize| format!("{:.2}", grid.west + (cell % grid.cols) as f64 * grid.step);
        let latitudes: Vec<String> = batch.iter().map(latitude).collect();
        let longitudes: Vec<String> = batch.iter().map(longitude).collect();
        let url = format!(
            "{open_meteo_url}?latitude={}&longitude={}&hourly=precipitation&forecast_hours={HOURS}&timezone=GMT",
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
                start = result["hourly"]["time"][0].as_str().map(|time| format!("{time}:00Z"));
            }
            let hourly = result["hourly"]["precipitation"].as_array().into_iter().flatten();
            for (hour, mm) in hourly.enumerate().take(HOURS) {
                if let Some(mm) = mm.as_f64() {
                    values[hour][*cell] = (mm * 10.0).round() as i32;
                }
            }
        }
        tracing::info!(
            points = ((index + 1) * POINTS_PER_REQUEST).min(grid.cells.len()),
            of = grid.cells.len(),
            "forecast batch"
        );
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
        "fuente": SOURCE,
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
}
