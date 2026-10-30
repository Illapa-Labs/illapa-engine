// SPDX-License-Identifier: AGPL-3.0-or-later
//! Settings from environment variables. Defaults are for local development.

use std::time::Duration;

const SENAMHI_WFS: &str = "https://idesep.senamhi.gob.pe/geoserver/wfs";

pub struct Config {
    /// `local`, `staging` or `production`; only logged, so each deployment says which one it is
    pub environment: String,
    pub database_url: String,
    pub address: String,
    pub senamhi_wfs_url: String,
    /// How often SENAMHI is asked again
    pub refresh_every: Duration,
    /// `false` turns off the background refresh (for example, to read an existing database only)
    pub refresh_enabled: bool,
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
            refresh_every: Duration::from_secs(var("REFRESH_SECS").and_then(|value| value.parse().ok()).unwrap_or(1800)),
            refresh_enabled: var("REFRESH_ENABLED").is_none_or(|value| value != "false"),
        }
    }
}
