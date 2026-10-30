// SPDX-License-Identifier: AGPL-3.0-or-later
//! The background refresh: asks SENAMHI for both warnings, works out which districts they touch and
//! how many people live in the warning areas, and saves it all in one transaction.

use anyhow::{Context, Result};
use serde_json::{Value, json};
use sqlx::Row;
use std::collections::{BTreeSet, HashMap, HashSet};

use crate::{
    AppState, geometry,
    senamhi::{self, POPULATED_CENTERS, RAIN_WARNING, RAVINE_WARNING, fold, level},
};

/// What one district gets from both warnings.
#[derive(Default)]
struct DistrictWarning {
    rain_level: i16,
    ravine_level: i16,
    /// Populated centers in ravine warning areas, by level (yellow, orange, red)
    populated_centers: [i32; 3],
    /// People living in those populated centers, by level (yellow, orange, red)
    people: [i64; 3],
}

pub struct Outcome {
    pub districts: usize,
    /// Districts named by SENAMHI that could not be matched with an INEI ubigeo, without repeats
    pub unmatched: BTreeSet<String>,
}

pub async fn run(state: &AppState) -> Result<Outcome> {
    let senamhi = &state.senamhi;
    let rain = senamhi.layer(RAIN_WARNING, "").await?;
    let features = rain["features"].as_array().cloned().unwrap_or_default();

    // The rain warning for the map: the same areas with less detail and only the fields the site uses.
    let display: Vec<Value> = features
        .iter()
        .map(|feature| {
            let properties: serde_json::Map<String, Value> = ["nivel", "fecha", "respons", "descripcio", "recomendac"]
                .iter()
                .map(|key| (key.to_string(), feature["properties"][*key].clone()))
                .collect();
            json!({ "type": "Feature", "properties": properties, "geometry": geometry::lighten(&feature["geometry"]) })
        })
        .collect();
    let refreshed_at = chrono::Utc::now();
    let aviso = json!({ "type": "FeatureCollection", "generated_at": refreshed_at.to_rfc3339(), "features": display });

    // INEI names and ubigeos, to match the warnings that only bring names.
    let rows = sqlx::query(
        "select d.ubigeo, dep.nombre as dep, p.nombre as prov, d.nombre as dist
           from distritos d join provincias p on p.ubigeo = d.provincia_ubigeo
                            join departamentos dep on dep.ubigeo = d.departamento_ubigeo",
    )
    .fetch_all(&state.pool)
    .await
    .context("INEI districts")?;
    let mut known = HashSet::new();
    let mut by_name = HashMap::new();
    for row in rows {
        let ubigeo = row.get::<String, _>("ubigeo").trim().to_string();
        let key = name_key(row.get("dep"), row.get("prov"), row.get("dist"));
        by_name.insert(key, ubigeo.clone());
        known.insert(ubigeo);
    }

    let mut districts: HashMap<String, DistrictWarning> = HashMap::new();
    let mut unmatched = BTreeSet::new();

    // 1. Rain warning: the districts each area touches, asked to SENAMHI's server.
    for feature in &features {
        let rain_level = level(&feature["properties"]["nivel"]);
        if rain_level < 2 {
            continue;
        }
        for ubigeo in senamhi
            .districts_touching(&feature["geometry"])
            .await
            .context("districts of the rain warning")?
        {
            if known.contains(&ubigeo) {
                let district = districts.entry(ubigeo).or_default();
                district.rain_level = district.rain_level.max(rain_level);
            } else {
                unmatched.insert(format!("ubigeo {ubigeo}"));
            }
        }
    }

    // 2. Ravine activation warning: its districts (by name) and the populated centers inside.
    let (ravines, centers) = tokio::try_join!(
        senamhi.layer(RAVINE_WARNING, "&propertyName=nivel,fecha,nombdep,nombprov,nombdist"),
        senamhi.layer(
            POPULATED_CENTERS,
            "&propertyName=nomb_dep,nomb_pro,nomb_dist,ubigeo,pob_total,nivel"
        ),
    )?;

    let mut ravine_date: Option<String> = None;
    for feature in ravines["features"].as_array().into_iter().flatten() {
        let p = &feature["properties"];
        if let Some(date) = p["fecha"].as_str()
            && ravine_date.as_deref().is_none_or(|latest| latest < date)
        {
            ravine_date = Some(date.to_string());
        }
        let text = |field: &str| p[field].as_str().unwrap_or_default();
        let key = name_key(text("nombdep"), text("nombprov"), text("nombdist"));
        match by_name.get(&key) {
            Some(ubigeo) => {
                let district = districts.entry(ubigeo.clone()).or_default();
                district.ravine_level = district.ravine_level.max(level(&p["nivel"]));
            }
            None => {
                unmatched.insert(key);
            }
        }
    }

    for feature in centers["features"].as_array().into_iter().flatten() {
        let p = &feature["properties"];
        let center_level = level(&p["nivel"]);
        if !(2..=4).contains(&center_level) {
            continue;
        }
        let text = |field: &str| p[field].as_str().unwrap_or_default();
        let ubigeo = senamhi::ubigeo(&p["ubigeo"]).filter(|ubigeo| known.contains(ubigeo)).or_else(|| {
            by_name
                .get(&name_key(text("nomb_dep"), text("nomb_pro"), text("nomb_dist")))
                .cloned()
        });
        let Some(ubigeo) = ubigeo else {
            unmatched.insert(name_key(text("nomb_dep"), text("nomb_pro"), text("nomb_dist")));
            continue;
        };
        let population = p["pob_total"]
            .as_i64()
            .or_else(|| p["pob_total"].as_str().and_then(|text| text.trim().parse().ok()))
            .unwrap_or(0);
        let district = districts.entry(ubigeo).or_default();
        let index = (center_level - 2) as usize;
        district.ravine_level = district.ravine_level.max(center_level);
        district.populated_centers[index] += 1;
        district.people[index] += population;
    }

    // Everything is saved together: either the new tables or the previous ones stay.
    let mut tx = state.pool.begin().await?;
    sqlx::query("delete from distrito_aviso").execute(&mut *tx).await?;
    for (ubigeo, district) in &districts {
        let people = district.people.map(|value| value.min(i32::MAX as i64) as i32);
        sqlx::query(
            "insert into distrito_aviso (ubigeo, nivel_lluvia, nivel_quebradas,
                    centros_poblados_amarillo, centros_poblados_naranja, centros_poblados_rojo,
                    personas_amarillo, personas_naranja, personas_rojo)
             values ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(ubigeo)
        .bind(district.rain_level)
        .bind(district.ravine_level)
        .bind(district.populated_centers[0])
        .bind(district.populated_centers[1])
        .bind(district.populated_centers[2])
        .bind(people[0])
        .bind(people[1])
        .bind(people[2])
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query(
        "insert into refresco (id, aviso, fecha_quebradas, actualizado) values (1, $1, $2, $3)
         on conflict (id) do update set aviso = excluded.aviso, fecha_quebradas = excluded.fecha_quebradas, actualizado = excluded.actualizado",
    )
    .bind(sqlx::types::Json(&aviso))
    .bind(&ravine_date)
    .bind(refreshed_at)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    *state.aviso.write().await = Some(aviso);
    Ok(Outcome {
        districts: districts.len(),
        unmatched,
    })
}

fn name_key(department: &str, province: &str, district: &str) -> String {
    format!("{}|{}|{}", fold(department), fold(province), fold(district))
}
