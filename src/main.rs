// SPDX-License-Identifier: AGPL-3.0-or-later
//! Illapa Engine: SENAMHI warnings by district, with their population, served as a JSON API.

use anyhow::{Context, Result};
use axum::{
    Router,
    http::{HeaderValue, header},
    routing::get,
};
use serde_json::Value;
use sqlx::{PgPool, postgres::PgPoolOptions};
use std::{sync::Arc, time::Duration};
use tokio::sync::RwLock;
use tower_http::{compression::CompressionLayer, cors::CorsLayer, set_header::SetResponseHeaderLayer, trace::TraceLayer};

mod config;
mod forecast;
mod geometry;
mod refresh;
mod routes;
mod senamhi;

const USER_AGENT: &str = concat!(
    "illapa-engine/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/Illapa-Labs/illapa-engine)"
);

#[derive(Clone)]
pub struct AppState {
    pool: PgPool,
    senamhi: senamhi::Senamhi,
    /// The last rain warning for the map; `None` until the first refresh, or a saved one, is available
    aviso: Arc<RwLock<Option<Value>>>,
    /// The last rain forecast; `None` until the first one, or a saved one, is available
    pronostico: Arc<RwLock<Option<Value>>>,
}

#[tokio::main]
async fn main() -> Result<()> {
    // A local `.env` file, if there is one; variables already set in the environment win.
    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,tower_http=warn".into()))
        .init();

    let config = config::Config::from_env();
    tracing::info!(environment = %config.environment, "starting illapa-engine");

    let pool = PgPoolOptions::new()
        .max_connections(5)
        .acquire_timeout(Duration::from_secs(5))
        .connect(&config.database_url)
        .await
        .context("could not connect to the database (see DATABASE_URL)")?;
    sqlx::migrate!("./migrations").run(&pool).await.context("migrations")?;

    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .user_agent(USER_AGENT)
        .build()?;

    // The last saved warning is served right away, while the background refresh gets a new one.
    let saved: Option<sqlx::types::Json<Value>> = sqlx::query_scalar("select aviso from refresco where id = 1")
        .fetch_optional(&pool)
        .await
        .context("saved warning")?;
    let saved_forecast: Option<sqlx::types::Json<Value>> = sqlx::query_scalar("select datos from pronostico where id = 1")
        .fetch_optional(&pool)
        .await
        .context("saved forecast")?;

    let state = AppState {
        pool,
        senamhi: senamhi::Senamhi::new(http.clone(), config.senamhi_wfs_url.clone()),
        aviso: Arc::new(RwLock::new(saved.map(|json| json.0))),
        pronostico: Arc::new(RwLock::new(saved_forecast.map(|json| json.0))),
    };

    if config.refresh_enabled {
        let background = state.clone();
        let every = config.refresh_every;
        tokio::spawn(async move {
            loop {
                match refresh::run(&background).await {
                    Ok(outcome) => {
                        tracing::info!(
                            districts = outcome.districts,
                            unmatched = outcome.unmatched.len(),
                            "warnings refreshed"
                        );
                        if !outcome.unmatched.is_empty() {
                            let names: Vec<&str> = outcome.unmatched.iter().map(String::as_str).collect();
                            tracing::warn!("SENAMHI districts without an INEI ubigeo: {}", names.join("; "));
                        }
                    }
                    Err(error) => tracing::warn!("refresh failed, the previous data stays: {error:#}"),
                }
                tokio::time::sleep(every).await;
            }
        });

        let background = state.clone();
        let every = config.forecast_every;
        let open_meteo_url = config.open_meteo_url.clone();
        tokio::spawn(async move {
            loop {
                let result = async {
                    let forecast = forecast::fetch(&http, &open_meteo_url).await?;
                    forecast::save(&background.pool, &forecast).await?;
                    *background.pronostico.write().await = Some(forecast);
                    anyhow::Ok(())
                }
                .await;
                match result {
                    Ok(()) => tracing::info!("forecast refreshed"),
                    Err(error) => tracing::warn!("forecast refresh failed, the previous one stays: {error:#}"),
                }
                tokio::time::sleep(every).await;
            }
        });
    } else {
        tracing::info!("background refresh is off (REFRESH_ENABLED=false)");
    }

    let app = Router::new()
        .route("/healthz", get(routes::health))
        .route("/v1/aviso", get(routes::aviso))
        .route("/v1/departamentos", get(routes::departamentos))
        .route("/v1/provincias", get(routes::provincias))
        .route("/v1/distritos", get(routes::distritos))
        .route("/v1/pronostico", get(routes::pronostico))
        // The data changes at most every refresh: browsers and CDNs can keep it for 5 minutes.
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=300"),
        ))
        .layer(CompressionLayer::new())
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&config.address)
        .await
        .with_context(|| format!("could not listen on {}", config.address))?;
    tracing::info!("listening on http://{}", config.address);
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
