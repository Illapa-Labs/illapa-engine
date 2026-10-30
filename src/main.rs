// SPDX-License-Identifier: AGPL-3.0-or-later
//! Illapa Engine: SENAMHI warnings by district, with their population, served as a JSON API.

use anyhow::{Context, Result};
use axum::{
    Router,
    error_handling::HandleErrorLayer,
    http::{HeaderValue, header},
    middleware,
    routing::get,
};
use serde_json::Value;
use sqlx::{PgPool, postgres::PgPoolOptions};
use std::{sync::Arc, time::Duration};
use tokio::sync::RwLock;
use tower::{ServiceBuilder, limit::ConcurrencyLimitLayer, load_shed::LoadShedLayer, timeout::TimeoutLayer};
use tower_http::{compression::CompressionLayer, cors::CorsLayer, set_header::SetResponseHeaderLayer, trace::TraceLayer};

mod config;
mod forecast;
mod geometry;
mod protection;
mod refresh;
mod routes;
mod senamhi;
mod snapshot;

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
    /// The department, province and district lists, rebuilt after every refresh
    snapshot: Arc<RwLock<Arc<snapshot::Snapshot>>>,
}

impl AppState {
    /// Rebuilds the lists from the database. If it fails, the previous lists stay.
    async fn reload_snapshot(&self) {
        match snapshot::Snapshot::load(&self.pool).await {
            Ok(snapshot) => *self.snapshot.write().await = Arc::new(snapshot),
            Err(error) => tracing::warn!("could not rebuild the lists, the previous ones stay: {error:#}"),
        }
    }
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
    let saved_forecast: Option<(sqlx::types::Json<Value>, chrono::DateTime<chrono::Utc>)> =
        sqlx::query_as("select datos, actualizado from pronostico where id = 1")
            .fetch_optional(&pool)
            .await
            .context("saved forecast")?;
    // How old the saved forecast is: a restart does not ask Open-Meteo again if it is still fresh
    let forecast_age = saved_forecast
        .as_ref()
        .and_then(|(_, saved_at)| (chrono::Utc::now() - *saved_at).to_std().ok());
    let saved_forecast = saved_forecast.map(|(json, _)| json);

    let state = AppState {
        pool,
        senamhi: senamhi::Senamhi::new(http.clone(), config.senamhi_wfs_url.clone(), config.senamhi_relay_key.clone()),
        aviso: Arc::new(RwLock::new(saved.map(|json| json.0))),
        pronostico: Arc::new(RwLock::new(saved_forecast.map(|json| json.0))),
        snapshot: Arc::new(RwLock::new(Arc::new(snapshot::Snapshot::default()))),
    };
    state.reload_snapshot().await;

    if config.refresh_enabled {
        let background = state.clone();
        let every = config.refresh_every;
        tokio::spawn(async move {
            loop {
                match refresh::run(&background).await {
                    Ok(outcome) => {
                        background.reload_snapshot().await;
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
            // Open-Meteo limits calls per hour, and every run asks for about 1 450 points: restarting the API (a
            // deploy, for example) must not ask again while the saved forecast is still fresh.
            if let Some(age) = forecast_age.filter(|age| *age < every) {
                let wait = every - age;
                tracing::info!(minutes = wait.as_secs() / 60, "saved forecast is fresh, next one later");
                tokio::time::sleep(wait).await;
            }
            loop {
                let result = async {
                    let forecast = forecast::fetch(&http, &open_meteo_url).await?;
                    forecast::save(&background.pool, &forecast).await?;
                    *background.pronostico.write().await = Some(forecast);
                    anyhow::Ok(())
                }
                .await;
                let wait = match result {
                    Ok(()) => {
                        tracing::info!("forecast refreshed");
                        every
                    }
                    Err(error) => {
                        // A failure (Open-Meteo's hourly limit, for example) is retried in an hour, not in 6
                        tracing::warn!("forecast refresh failed, the previous one stays: {error:#}");
                        every.min(Duration::from_secs(3600))
                    }
                };
                tokio::time::sleep(wait).await;
            }
        });
    } else {
        tracing::info!("background refresh is off (REFRESH_ENABLED=false)");
    }

    // Routes with filters, and routes that take no parameters at all (see protection.rs)
    let filtered = Router::new()
        .route("/v1/provincias", get(routes::provincias))
        .route("/v1/distritos", get(routes::distritos));
    let fixed = Router::new()
        .route("/v1/aviso", get(routes::aviso))
        .route("/v1/departamentos", get(routes::departamentos))
        .route("/v1/pronostico", get(routes::pronostico))
        .route_layer(middleware::from_fn(protection::reject_query));
    let mut data = filtered.merge(fixed);
    match &config.origin_secret {
        Some(secret) => {
            if !config.origin_enforce {
                tracing::warn!("ORIGIN_CHECK=report: requests without the Cloudflare header are logged, not refused");
            }
            let check = protection::OriginCheck {
                secret: Arc::from(secret.as_str()),
                enforce: config.origin_enforce,
            };
            data = data.route_layer(middleware::from_fn_with_state(check, protection::require_origin_secret));
        }
        None => tracing::warn!("ORIGIN_SECRET is not set: the data routes answer requests from anywhere, not only Cloudflare"),
    }

    let app = data
        .route("/healthz", get(routes::health))
        .layer(
            ServiceBuilder::new()
                .layer(HandleErrorLayer::new(protection::overloaded))
                .layer(LoadShedLayer::new())
                .layer(ConcurrencyLimitLayer::new(protection::MAX_CONCURRENT_REQUESTS))
                .layer(TimeoutLayer::new(protection::REQUEST_TIMEOUT)),
        )
        // Each data route sets how long it can be kept (routes.rs); anything else is not cached.
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store"),
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
