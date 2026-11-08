// SPDX-License-Identifier: AGPL-3.0-or-later
//! Settings from environment variables. Defaults are for local development.

use std::time::Duration;

const SENAMHI_WFS: &str = "https://idesep.senamhi.gob.pe/geoserver/wfs";
const OPEN_METEO: &str = "https://api.open-meteo.com/v1/forecast";

pub struct Config {
    /// `local`, `staging` or `production`; only logged, so each deployment says which one it is
    pub environment: String,
    pub database_url: String,
    pub address: String,
    pub senamhi_wfs_url: String,
    /// Key for the Cloudflare relay (`relay/`), when `SENAMHI_WFS_URL` points to it
    pub senamhi_relay_key: Option<String>,
    /// How often SENAMHI is asked again
    pub refresh_every: Duration,
    /// `false` turns off the background refresh (for example, to read an existing database only)
    pub refresh_enabled: bool,
    /// Secret that Cloudflare adds to every request; when set, requests without it are refused (see protection.rs)
    pub origin_secret: Option<String>,
    /// `false` with `ORIGIN_CHECK=report`: requests without the secret are only logged, to try a new Cloudflare rule
    pub origin_enforce: bool,
    pub open_meteo_url: String,
    /// How often the forecast is asked again
    pub forecast_every: Duration,
    /// Whether the GOES-19 cloud images are kept up to date (see clouds.rs). By default they follow
    /// `REFRESH_ENABLED`; `CLOUDS_ENABLED=true` or `false` overrides it
    pub clouds_enabled: bool,
    /// Whether SENAMHI's river flood warnings (SONICS-PBI, see floods.rs) are kept up to date. Like the clouds, they
    /// follow `REFRESH_ENABLED` unless `FLOODS_ENABLED=true` or `false` says otherwise
    pub floods_enabled: bool,
    /// How often SENAMHI is asked whether there is a new flood warning run (a few hundred bytes each time)
    pub floods_every: Duration,
}

impl Config {
    pub fn from_env() -> Self {
        let var = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
        let user = var("USER").unwrap_or_else(|| "postgres".into());
        Self {
            environment: var("ILLAPA_ENV").unwrap_or_else(|| "local".into()),
            database_url: var("DATABASE_URL").unwrap_or_else(|| format!("postgres://{user}@localhost/illapa")),
            address: var("ILLAPA_ADDR").unwrap_or_else(|| "127.0.0.1:8080".into()),
            senamhi_wfs_url: var("SENAMHI_WFS_URL").unwrap_or_else(|| SENAMHI_WFS.into()),
            senamhi_relay_key: var("SENAMHI_RELAY_KEY"),
            refresh_every: Duration::from_secs(var("REFRESH_SECS").and_then(|value| value.parse().ok()).unwrap_or(1800)),
            refresh_enabled: var("REFRESH_ENABLED").is_none_or(|value| value != "false"),
            origin_secret: var("ORIGIN_SECRET"),
            origin_enforce: var("ORIGIN_CHECK").is_none_or(|value| value != "report"),
            open_meteo_url: var("OPEN_METEO_URL").unwrap_or_else(|| OPEN_METEO.into()),
            forecast_every: Duration::from_secs(var("FORECAST_REFRESH_SECS").and_then(|value| value.parse().ok()).unwrap_or(43_200)),
            clouds_enabled: match var("CLOUDS_ENABLED").as_deref() {
                Some("true") => true,
                Some("false") => false,
                _ => var("REFRESH_ENABLED").is_none_or(|value| value != "false"),
            },
            floods_enabled: match var("FLOODS_ENABLED").as_deref() {
                Some("true") => true,
                Some("false") => false,
                _ => var("REFRESH_ENABLED").is_none_or(|value| value != "false"),
            },
            floods_every: Duration::from_secs(var("FLOODS_CHECK_SECS").and_then(|value| value.parse().ok()).unwrap_or(3600)),
        }
    }
}
