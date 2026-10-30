// SPDX-License-Identifier: AGPL-3.0-or-later
//! Client for SENAMHI's public WFS server (IDESEP). No account is needed.

use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::time::{Duration, Instant};

use crate::geometry;

/// Rain warning: 24 h forecast by area.
pub const RAIN_WARNING: &str = "g_prono_pp_24h:view_aviso24h";
/// Ravine activation warning, by district (names only).
pub const RAVINE_WARNING: &str = "g_acti_quebrada:view_av_activ_qdra";
/// Populated centers inside areas of the ravine activation warning, with their population.
pub const POPULATED_CENTERS: &str = "g_acti_quebrada:ccpp_filter";
/// District boundaries, used to ask which districts an area touches.
const DISTRICTS: &str = "g_carto_fundamento:distritos";

/// The server sometimes takes long or drops the connection: each request is tried this many times.
const ATTEMPTS: u32 = 2;
const LAYER_TIMEOUT: Duration = Duration::from_secs(90);
const SPATIAL_QUERY_TIMEOUT: Duration = Duration::from_secs(120);

/// Header with the key shared with the Cloudflare relay (`relay/`), when the API reaches SENAMHI through it.
const RELAY_KEY_HEADER: &str = "x-illapa-relay-key";

#[derive(Clone)]
pub struct Senamhi {
    http: reqwest::Client,
    wfs_url: String,
    /// Key for the relay; `None` when the API talks to SENAMHI directly
    relay_key: Option<String>,
}

impl Senamhi {
    pub fn new(http: reqwest::Client, wfs_url: String, relay_key: Option<String>) -> Self {
        Self { http, wfs_url, relay_key }
    }

    /// Adds the relay key to a request, if there is one.
    fn with_key(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.relay_key {
            Some(key) => request.header(RELAY_KEY_HEADER, key),
            None => request,
        }
    }

    /// A whole layer as GeoJSON. `extra` is appended to the query string (for example `&propertyName=...`).
    pub async fn layer(&self, type_name: &str, extra: &str) -> Result<Value> {
        let url = format!(
            "{}?service=WFS&version=1.0.0&request=GetFeature&typeName={type_name}&maxFeatures=20000&outputFormat=application/json&srsName=EPSG:4326{extra}",
            self.wfs_url
        );
        let mut attempt = 1;
        loop {
            let started = Instant::now();
            let result: Result<Value> = async {
                let response = self
                    .with_key(self.http.get(&url))
                    .timeout(LAYER_TIMEOUT)
                    .send()
                    .await?
                    .error_for_status()?;
                Ok(response.json().await?)
            }
            .await;
            match result {
                Ok(value) => {
                    tracing::info!(
                        layer = type_name,
                        seconds = started.elapsed().as_secs_f32(),
                        "SENAMHI layer downloaded"
                    );
                    return Ok(value);
                }
                Err(error) if attempt < ATTEMPTS => {
                    tracing::warn!(
                        layer = type_name,
                        attempt,
                        seconds = started.elapsed().as_secs_f32(),
                        "SENAMHI layer failed: {error:#}"
                    );
                    attempt += 1;
                }
                Err(error) => return Err(error).with_context(|| format!("SENAMHI layer {type_name}")),
            }
        }
    }

    /// The ubigeos (6 digits) of the districts that an area touches, by a spatial query to the server.
    pub async fn districts_touching(&self, area: &Value) -> Result<Vec<String>> {
        let Some(wkt) = geometry::query_wkt(area) else {
            return Ok(Vec::new());
        };
        let filter = format!("INTERSECTS(geom, {wkt})");
        let form = [
            ("service", "WFS"),
            ("version", "1.0.0"),
            ("request", "GetFeature"),
            ("typeName", DISTRICTS),
            ("outputFormat", "application/json"),
            ("propertyName", "iddist"),
            ("maxFeatures", "3000"),
            ("cql_filter", filter.as_str()),
        ];
        let response = self
            .with_key(self.http.post(&self.wfs_url))
            .timeout(SPATIAL_QUERY_TIMEOUT)
            .form(&form)
            .send()
            .await?;
        if !response.status().is_success() {
            bail!("SENAMHI districts query: HTTP {}", response.status());
        }
        let body: Value = response.json().await.context("SENAMHI districts query")?;
        Ok(body["features"]
            .as_array()
            .map(|features| {
                features
                    .iter()
                    .filter_map(|feature| ubigeo(&feature["properties"]["iddist"]))
                    .flat_map(inei_districts)
                    .collect()
            })
            .unwrap_or_default())
    }
}

/// SENAMHI's boundary layer has areas that are not one INEI district. `120699` ("MAZAMARI-PANGOA") covers
/// Mazamari and Pangoa (Satipo, Junín), which the layer does not have separately: a warning there is for both.
const BOUNDARY_ALIASES: &[(&str, &[&str])] = &[("120699", &["120604", "120606"])];

/// The INEI districts behind a ubigeo of SENAMHI's boundary layer: usually the same one.
fn inei_districts(ubigeo: String) -> Vec<String> {
    match BOUNDARY_ALIASES.iter().find(|(alias, _)| *alias == ubigeo) {
        Some((_, districts)) => districts.iter().map(|district| district.to_string()).collect(),
        None => vec![ubigeo],
    }
}

/// A ubigeo from SENAMHI data, which may come as a number or a string without leading zeros.
pub fn ubigeo(value: &Value) -> Option<String> {
    let text = match value {
        Value::String(text) => text.trim().to_string(),
        Value::Number(number) => number.to_string(),
        _ => return None,
    };
    (!text.is_empty() && text.len() <= 6 && text.chars().all(|c| c.is_ascii_digit())).then(|| format!("{text:0>6}"))
}

/// A SENAMHI level from values like `"Nivel 3"`, `3` or `"3"`; 0 when there is none.
pub fn level(value: &Value) -> i16 {
    let text = match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    text.chars().filter(char::is_ascii_digit).collect::<String>().parse().unwrap_or(0)
}

/// Upper case without accents, to match SENAMHI names with INEI names.
pub fn fold(name: &str) -> String {
    name.trim()
        .to_uppercase()
        .chars()
        .map(|c| match c {
            'Á' => 'A',
            'É' => 'E',
            'Í' => 'I',
            'Ó' => 'O',
            'Ú' | 'Ü' => 'U',
            'Ñ' => 'N',
            other => other,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn levels_are_read_from_text_and_numbers() {
        assert_eq!(level(&json!("Nivel 3")), 3);
        assert_eq!(level(&json!(4)), 4);
        assert_eq!(level(&json!(null)), 0);
    }

    #[test]
    fn ubigeos_get_their_leading_zeros() {
        assert_eq!(ubigeo(&json!(10101)).as_deref(), Some("010101"));
        assert_eq!(ubigeo(&json!("150101")).as_deref(), Some("150101"));
        assert_eq!(ubigeo(&json!("lima")), None);
    }

    #[test]
    fn mazamari_pangoa_is_both_districts() {
        assert_eq!(inei_districts("120699".into()), vec!["120604", "120606"]);
        assert_eq!(inei_districts("150101".into()), vec!["150101"]);
    }

    #[test]
    fn names_fold_accents_and_case() {
        assert_eq!(fold(" Áncash "), "ANCASH");
        assert_eq!(fold("Cañete"), "CANETE");
    }
}
