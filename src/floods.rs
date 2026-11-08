// SPDX-License-Identifier: AGPL-3.0-or-later
//! River flood warnings from SENAMHI's SONICS-PBI ("pronósticos basados en impacto"): the subbasins where a river
//! may flood, by level (1 yellow, 2 orange, 3 red), for the current state and the next seven days, and what lies in
//! them: populated centers, schools, health facilities and roads.
//!
//! SENAMHI publishes a new run once a day (around 13:00 in Lima). Day 0 ("estado actual") is the state observed up
//! to the day before the run; days 1 to 7 are the forecast from the run's day on. Each day is one WFS layer with one
//! `MultiPolygon` per level (one polygon per subbasin), about 12.5 MB, and six exposure layers with one point (or
//! road piece) per item and its level.
//!
//! Every hour the API asks only for the levels and dates of the eight days (a few hundred bytes). When they change,
//! it downloads the eight days again: the areas are simplified for the map (about 340 KB per day) and the exposure
//! layers are read without their geometry and counted by district (populated centers, schools and health facilities
//! carry the district's ubigeo). Roads only carry their province (departmental and neighbourhood roads) or their
//! department (national roads), so their kilometres are counted there. The result is saved, so a restart serves it
//! right away.

use anyhow::{Context, Result, bail};
use chrono::{Days, NaiveDate, Utc};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::PgPool;
use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    sync::Arc,
    time::Duration,
};
use tokio::sync::RwLock;

use crate::{
    geometry::{self, Polygon},
    senamhi::{Senamhi, level},
};

/// The current state (day 0) and the seven forecast days.
pub const DAYS: usize = 8;
const WORKSPACE: &str = "dhi-seh-sonics";
/// Simplification for the map, in degrees (about 550 m). The areas come from a grid of 1/120° (about 900 m), so this
/// keeps its steps.
const TOLERANCE: f64 = 0.005;
/// Coordinates for the map keep 3 decimals (about 110 m).
const SCALE: f64 = 1e3;

pub const SOURCE: &str = "Avisos de inundación de ríos (SONICS-PBI) y lo expuesto: SENAMHI. No es un sistema oficial de alerta.";
/// SENAMHI's own warning about how to read the map, in short.
const NOTE: &str = "El área coloreada es la subcuenca donde puede haber impacto por inundación del río; el área exacta \
    que se inunda puede variar dentro de ella. El estado actual es lo observado hasta el día anterior a la emisión; \
    los días 1 a 7 son el pronóstico desde el día de la emisión.";

/// What each level means, as SENAMHI's Observatorio describes it (servicios.senamhi.gob.pe, `hydrology/sonics`).
/// Colors are the Observatorio's.
pub const LEVELS: [(i16, &str, &str, &str); 3] = [
    (
        1,
        "Amarillo",
        "#facc15",
        "Probables condiciones que podrían afectar las actividades en el río y zonas aledañas.",
    ),
    (
        2,
        "Naranja",
        "#f97316",
        "Se esperan condiciones de riesgo para actividades en el río y zonas aledañas, con probable erosión de \
         márgenes y afectación en infraestructura dentro del ámbito directo o cercano al río.",
    ),
    (
        3,
        "Rojo",
        "#dc2626",
        "Previsión de desborde del río y erosión en los márgenes. Posible inundación en zonas pobladas y agrícolas \
         más bajas; afectación en infraestructura dentro del ámbito directo o cercano al río.",
    ),
];
const RECOMMENDATION: &str = "Se recomienda a la población tomar las precauciones correspondientes y evitar realizar \
    actividades cercanas al río.";

/// The last run, ready to serve: the summary and the map of each day.
pub struct Floods {
    pub resumen: Value,
    /// One GeoJSON per day, day 0 first
    pub dias: Vec<Value>,
    /// The levels and dates of the eight days, to tell whether SENAMHI published a new run
    pub firma: String,
}

pub type Slot = Arc<RwLock<Option<Arc<Floods>>>>;

/// The WFS layer of a product (`SONICS`, `SONICS-CCPP`, ...) for one day.
fn layer_name(product: &str, day: usize) -> String {
    if day == 0 {
        format!("{WORKSPACE}:{product}")
    } else {
        format!("{WORKSPACE}:{product}-PRONOSTICO-{day:02}")
    }
}

/// A SONICS date such as `"2026-11-05Z"`.
fn parse_date(text: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(text.trim().get(..10)?, "%Y-%m-%d").ok()
}

/// The levels and dates of one day's areas, in a stable order.
type DayDates = Vec<(i16, Option<NaiveDate>)>;

/// One line per day: the signature that changes when SENAMHI publishes a new run.
fn signature(days: &[DayDates]) -> String {
    days.iter()
        .map(|day| {
            let mut parts: Vec<String> = day
                .iter()
                .map(|(level, date)| format!("{level}:{}", date.map(|date| date.to_string()).unwrap_or_default()))
                .collect();
            parts.sort();
            parts.join(",")
        })
        .collect::<Vec<_>>()
        .join("|")
}

/// The date of day 0 ("estado actual"), from the dates of every day: day `n` must be `n` days later. If they do not
/// agree, SENAMHI is publishing a new run right now and the layers are mixed: better to wait. `None` when no day has
/// any area (there is nothing to date).
fn base_date(days: &[DayDates]) -> Result<Option<NaiveDate>> {
    let bases: BTreeSet<NaiveDate> = days
        .iter()
        .enumerate()
        .flat_map(|(day, dates)| {
            dates
                .iter()
                .filter_map(move |(_, date)| date.and_then(|date| date.checked_sub_days(Days::new(day as u64))))
        })
        .collect();
    if bases.len() > 1 {
        bail!("the SONICS days are from different runs ({bases:?}); SENAMHI may be publishing a new one");
    }
    Ok(bases.into_iter().next())
}

fn dates_of(features: &Value) -> DayDates {
    features["features"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|feature| {
            let p = &feature["properties"];
            (level(&p["nivel"]), p["fecha"].as_str().and_then(parse_date))
        })
        .collect()
}

/// Only the levels and dates of the eight days: a few hundred bytes, to check for a new run.
async fn check(senamhi: &Senamhi) -> Result<Vec<DayDates>> {
    let mut days = Vec::with_capacity(DAYS);
    for day in 0..DAYS {
        let layer = senamhi.layer(&layer_name("SONICS", day), "&propertyName=nivel,fecha").await?;
        days.push(dates_of(&layer));
    }
    Ok(days)
}

/// One day's areas, read into a typed shape: the whole layer as a `Value` would take several times the memory.
#[derive(Deserialize)]
struct Areas {
    features: Vec<Area>,
}

#[derive(Deserialize)]
struct Area {
    properties: AreaProperties,
    geometry: Option<Shape>,
}

#[derive(Deserialize)]
struct AreaProperties {
    nivel: Value,
    fecha: Option<String>,
}

#[derive(Deserialize)]
#[serde(tag = "type", content = "coordinates")]
enum Shape {
    Polygon(Vec<Vec<Vec<f64>>>),
    MultiPolygon(Vec<Vec<Vec<Vec<f64>>>>),
}

impl Shape {
    fn polygons(self) -> Vec<Polygon> {
        let polygon = |rings: Vec<Vec<Vec<f64>>>| -> Polygon {
            rings
                .into_iter()
                .map(|ring| {
                    ring.into_iter()
                        .filter(|point| point.len() >= 2)
                        .map(|point| [point[0], point[1]])
                        .collect()
                })
                .collect()
        };
        match self {
            Self::Polygon(rings) => vec![polygon(rings)],
            Self::MultiPolygon(polygons) => polygons.into_iter().map(polygon).collect(),
        }
    }
}

/// Counts by level: index 0 is level 1 (yellow), 1 is orange, 2 is red.
type ByLevel = [u32; 3];
/// Metres by level, the same way.
type MetresByLevel = [f64; 3];

fn level_index(value: &Value) -> Option<usize> {
    let level = level(value);
    (1..=3).contains(&level).then(|| (level - 1) as usize)
}

/// A code with its leading zeros: ubigeos have 6 digits, provinces 4 and departments 2.
fn code(value: &Value, width: usize) -> Option<String> {
    let text = match value {
        Value::String(text) => text.trim().to_string(),
        Value::Number(number) => number.to_string(),
        _ => return None,
    };
    (!text.is_empty() && text.len() <= width && text.chars().all(|c| c.is_ascii_digit())).then(|| format!("{text:0>width$}"))
}

/// What one district has in the warning areas on one day.
#[derive(Default, Clone, PartialEq, Debug)]
struct DistrictDay {
    centros_poblados: ByLevel,
    instituciones_educativas: ByLevel,
    establecimientos_salud: ByLevel,
}

impl DistrictDay {
    fn highest_level(&self) -> usize {
        (0..3)
            .rev()
            .find(|&i| self.centros_poblados[i] + self.instituciones_educativas[i] + self.establecimientos_salud[i] > 0)
            .map_or(0, |i| i + 1)
    }
}

/// The exposure layers of one day, already counted.
#[derive(Default)]
struct Exposure {
    districts: BTreeMap<String, DistrictDay>,
    /// Departmental and neighbourhood roads, in metres, by province
    province_roads: BTreeMap<String, [MetresByLevel; 2]>,
    /// National roads, in metres, by department
    department_roads: BTreeMap<String, MetresByLevel>,
    /// Totals for the whole country, including items whose district is unknown
    centros_poblados: ByLevel,
    instituciones_educativas: ByLevel,
    establecimientos_salud: ByLevel,
    /// National, departmental and neighbourhood roads, in metres
    roads: [MetresByLevel; 3],
    /// Ubigeos that are not INEI districts
    unknown: BTreeSet<String>,
}

fn features(layer: &Value) -> impl Iterator<Item = &Value> {
    layer["features"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|feature| &feature["properties"])
}

/// Which district count a layer adds to.
#[derive(Clone, Copy)]
enum Item {
    PopulatedCenter,
    School,
    HealthFacility,
}

impl Exposure {
    fn count_points(&mut self, layer: &Value, ubigeo_field: &str, item: Item, known: &HashSet<String>) {
        for p in features(layer) {
            let Some(index) = level_index(&p["nivel"]) else { continue };
            let total = match item {
                Item::PopulatedCenter => &mut self.centros_poblados,
                Item::School => &mut self.instituciones_educativas,
                Item::HealthFacility => &mut self.establecimientos_salud,
            };
            total[index] += 1;
            let Some(ubigeo) = code(&p[ubigeo_field], 6) else { continue };
            if !known.contains(&ubigeo) {
                self.unknown.insert(ubigeo);
                continue;
            }
            let district = self.districts.entry(ubigeo).or_default();
            let counts = match item {
                Item::PopulatedCenter => &mut district.centros_poblados,
                Item::School => &mut district.instituciones_educativas,
                Item::HealthFacility => &mut district.establecimientos_salud,
            };
            counts[index] += 1;
        }
    }

    /// `kind`: 0 national (by department), 1 departmental and 2 neighbourhood (by province).
    fn count_roads(&mut self, layer: &Value, kind: usize) {
        for p in features(layer) {
            let Some(index) = level_index(&p["nivel"]) else { continue };
            let metres = p["longitud"].as_f64().unwrap_or(0.0);
            self.roads[kind][index] += metres;
            if kind == 0 {
                if let Some(department) = code(&p["coddep"], 2) {
                    self.department_roads.entry(department).or_default()[index] += metres;
                }
            } else if let Some(province) = code(&p["codprov"], 4) {
                self.province_roads.entry(province).or_default()[kind - 1][index] += metres;
            }
        }
    }
}

fn kilometres(metres: &MetresByLevel) -> Value {
    json!(metres.map(|metres| (metres / 100.0).round() / 10.0))
}

/// Downloads and counts one day: its map and what lies in its warning areas.
async fn day(senamhi: &Senamhi, day: usize, known: &HashSet<String>) -> Result<(Value, DayDates, ByLevel, Exposure)> {
    let areas: Areas = senamhi.layer_as(&layer_name("SONICS", day), "").await?;
    let mut dates = Vec::new();
    let mut subbasins: ByLevel = [0; 3];
    let mut map_features = Vec::new();
    for area in areas.features {
        let level_value = level(&area.properties.nivel);
        dates.push((level_value, area.properties.fecha.as_deref().and_then(parse_date)));
        let Some(index) = level_index(&area.properties.nivel) else {
            continue;
        };
        let polygons = area.geometry.map(Shape::polygons).unwrap_or_default();
        // One polygon per subbasin, as SENAMHI counts them
        subbasins[index] += polygons.len() as u32;
        map_features.push((
            level_value,
            json!({
                "type": "Feature",
                "properties": { "nivel": level_value, "subcuencas": polygons.len() },
                "geometry": geometry::lighten_polygons(&polygons, TOLERANCE, SCALE),
            }),
        ));
    }
    // Higher levels last, so they are drawn on top
    map_features.sort_by_key(|(level, _)| *level);

    let names = [
        "SONICS-CCPP",
        "SONICS-IIEE",
        "SONICS-EESS",
        "SONICS-VIAL-NACIONAL",
        "SONICS-VIAL-DEPARTAMENTAL",
        "SONICS-VIAL-VECINAL",
    ]
    .map(|product| layer_name(product, day));
    let (centers, schools, health, national, departmental, neighbourhood) = tokio::try_join!(
        senamhi.layer(&names[0], "&propertyName=ubigeo,nivel"),
        senamhi.layer(&names[1], "&propertyName=iddistrito,nivel"),
        senamhi.layer(&names[2], "&propertyName=estubigeo,nivel"),
        senamhi.layer(&names[3], "&propertyName=coddep,longitud,nivel"),
        senamhi.layer(&names[4], "&propertyName=codprov,longitud,nivel"),
        senamhi.layer(&names[5], "&propertyName=codprov,longitud,nivel"),
    )?;
    let mut exposure = Exposure::default();
    exposure.count_points(&centers, "ubigeo", Item::PopulatedCenter, known);
    exposure.count_points(&schools, "iddistrito", Item::School, known);
    exposure.count_points(&health, "estubigeo", Item::HealthFacility, known);
    exposure.count_roads(&national, 0);
    exposure.count_roads(&departmental, 1);
    exposure.count_roads(&neighbourhood, 2);

    let map = json!({ "type": "FeatureCollection", "features": map_features.into_iter().map(|(_, feature)| feature).collect::<Vec<_>>() });
    Ok((map, dates, subbasins, exposure))
}

/// Downloads the eight days and puts together what the API serves.
async fn build(senamhi: &Senamhi, known: &HashSet<String>) -> Result<Floods> {
    let mut maps = Vec::with_capacity(DAYS);
    let mut all_dates = Vec::with_capacity(DAYS);
    let mut days = Vec::with_capacity(DAYS);
    for n in 0..DAYS {
        // One day at a time: each one is about 12.5 MB of areas
        let (map, dates, subbasins, exposure) = day(senamhi, n, known).await.with_context(|| format!("SONICS day {n}"))?;
        maps.push(map);
        all_dates.push(dates);
        days.push((subbasins, exposure));
    }
    let base = base_date(&all_dates)?;
    let date_of = |n: usize| {
        base.and_then(|base| base.checked_add_days(Days::new(n as u64)))
            .map(|date| date.to_string())
    };

    let mut unknown = BTreeSet::new();
    let mut districts: BTreeMap<&str, Vec<Value>> = BTreeMap::new();
    let mut provinces: BTreeMap<&str, Vec<Value>> = BTreeMap::new();
    let mut departments: BTreeMap<&str, Vec<Value>> = BTreeMap::new();
    let mut summary_days = Vec::with_capacity(DAYS);
    for (n, (subbasins, exposure)) in days.iter().enumerate() {
        unknown.extend(exposure.unknown.iter().cloned());
        for (ubigeo, district) in &exposure.districts {
            districts.entry(ubigeo).or_default().push(json!({
                "dia": n,
                "nivel": district.highest_level(),
                "centros_poblados": district.centros_poblados,
                "instituciones_educativas": district.instituciones_educativas,
                "establecimientos_salud": district.establecimientos_salud,
            }));
        }
        for (province, [departmental, neighbourhood]) in &exposure.province_roads {
            provinces.entry(province).or_default().push(json!({
                "dia": n,
                "vias_km": { "departamental": kilometres(departmental), "vecinal": kilometres(neighbourhood) },
            }));
        }
        for (department, national) in &exposure.department_roads {
            departments
                .entry(department)
                .or_default()
                .push(json!({ "dia": n, "vias_km": { "nacional": kilometres(national) } }));
        }
        summary_days.push(json!({
            "dia": n,
            "fecha": date_of(n),
            "pronostico": n > 0,
            "mapa": format!("/v1/inundaciones/{n}"),
            "subcuencas": subbasins,
            "centros_poblados": exposure.centros_poblados,
            "instituciones_educativas": exposure.instituciones_educativas,
            "establecimientos_salud": exposure.establecimientos_salud,
            "vias_km": {
                "nacional": kilometres(&exposure.roads[0]),
                "departamental": kilometres(&exposure.roads[1]),
                "vecinal": kilometres(&exposure.roads[2]),
            },
        }));
    }
    if !unknown.is_empty() {
        let list: Vec<&str> = unknown.iter().map(String::as_str).collect();
        tracing::warn!("SONICS ubigeos that are not INEI districts: {}", list.join(", "));
    }

    let levels: Vec<Value> = LEVELS
        .iter()
        .map(|(level, name, color, hazard)| {
            json!({ "nivel": level, "nombre": name, "color": color, "peligro": hazard, "recomendacion": RECOMMENDATION })
        })
        .collect();
    let resumen = json!({
        "fuente": SOURCE,
        "nota": NOTE,
        "actualizado": Utc::now().to_rfc3339(),
        "observado_hasta": date_of(0),
        "niveles": levels,
        "dias": summary_days,
        "distritos": districts,
        "provincias": provinces,
        "departamentos": departments,
    });
    let dias = maps
        .into_iter()
        .enumerate()
        .map(|(n, mut map)| {
            if let Some(object) = map.as_object_mut() {
                object.insert("dia".into(), json!(n));
                object.insert("fecha".into(), json!(date_of(n)));
            }
            map
        })
        .collect();
    Ok(Floods {
        resumen,
        dias,
        firma: signature(&all_dates),
    })
}

/// The last run saved, if there is one.
pub async fn load(pool: &PgPool) -> Result<Option<Floods>> {
    let saved: Option<(sqlx::types::Json<Value>, sqlx::types::Json<Vec<Value>>, String)> =
        sqlx::query_as("select resumen, dias, firma from inundaciones where id = 1")
            .fetch_optional(pool)
            .await
            .context("saved river flood warnings")?;
    Ok(saved
        .filter(|(_, dias, _)| dias.0.len() == DAYS)
        .map(|(resumen, dias, firma)| Floods {
            resumen: resumen.0,
            dias: dias.0,
            firma,
        }))
}

async fn save(pool: &PgPool, floods: &Floods) -> Result<()> {
    sqlx::query(
        "insert into inundaciones (id, resumen, dias, firma, actualizado) values (1, $1, $2, $3, now())
         on conflict (id) do update set resumen = excluded.resumen, dias = excluded.dias, firma = excluded.firma,
                                        actualizado = excluded.actualizado",
    )
    .bind(sqlx::types::Json(&floods.resumen))
    .bind(sqlx::types::Json(&floods.dias))
    .bind(&floods.firma)
    .execute(pool)
    .await
    .context("saving the river flood warnings")?;
    Ok(())
}

/// Checks for a new run and, if there is one, downloads it. `true` when the data changed.
async fn refresh(pool: &PgPool, senamhi: &Senamhi, slot: &Slot) -> Result<bool> {
    let dates = check(senamhi).await?;
    base_date(&dates)?;
    let firma = signature(&dates);
    if slot.read().await.as_ref().is_some_and(|floods| floods.firma == firma) {
        return Ok(false);
    }
    let known: HashSet<String> = sqlx::query_scalar::<_, String>("select ubigeo from distritos")
        .fetch_all(pool)
        .await
        .context("INEI districts")?
        .into_iter()
        .map(|ubigeo| ubigeo.trim().to_string())
        .collect();
    let floods = build(senamhi, &known).await?;
    save(pool, &floods).await?;
    *slot.write().await = Some(Arc::new(floods));
    Ok(true)
}

/// The background loop: checks every `every` (an hour by default) and downloads only new runs.
pub async fn run(pool: PgPool, senamhi: Senamhi, slot: Slot, every: Duration) {
    loop {
        match refresh(&pool, &senamhi, &slot).await {
            Ok(true) => {
                let observed = slot.read().await.as_ref().map(|floods| floods.resumen["observado_hasta"].clone());
                tracing::info!(observado_hasta = ?observed, "river flood warnings refreshed");
            }
            Ok(false) => tracing::debug!("river flood warnings unchanged"),
            Err(error) => tracing::warn!("river flood warnings refresh failed, the previous ones stay: {error:#}"),
        }
        tokio::time::sleep(every).await;
    }
}

/// Keeps only the fields a map needs, for tests.
#[cfg(test)]
fn properties(map: &Value) -> Vec<serde_json::Map<String, Value>> {
    map["features"]
        .as_array()
        .unwrap()
        .iter()
        .map(|feature| feature["properties"].as_object().unwrap().clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(text: &str) -> Option<NaiveDate> {
        parse_date(text)
    }

    #[test]
    fn layer_names_follow_senamhi() {
        assert_eq!(layer_name("SONICS", 0), "dhi-seh-sonics:SONICS");
        assert_eq!(layer_name("SONICS", 7), "dhi-seh-sonics:SONICS-PRONOSTICO-07");
        assert_eq!(layer_name("SONICS-CCPP", 1), "dhi-seh-sonics:SONICS-CCPP-PRONOSTICO-01");
    }

    #[test]
    fn dates_drop_the_zone_suffix() {
        assert_eq!(date("2026-11-05Z"), NaiveDate::from_ymd_opt(2026, 11, 5));
        assert_eq!(date("2026-11-05"), NaiveDate::from_ymd_opt(2026, 11, 5));
        assert_eq!(date("ayer"), None);
    }

    #[test]
    fn day_zero_is_dated_from_every_day_and_mixed_runs_wait() {
        let run = |dates: &[&str]| -> Vec<DayDates> { dates.iter().map(|text| vec![(1, date(text)), (3, date(text))]).collect() };
        // Day n is n days after day 0, as SENAMHI published on 2026-11-06
        let days = run(&["2026-11-05Z", "2026-11-06Z", "2026-11-07Z"]);
        assert_eq!(base_date(&days).unwrap(), NaiveDate::from_ymd_opt(2026, 11, 5));
        // A day without areas has no date, and does not get in the way
        let mut with_empty = days.clone();
        with_empty.push(Vec::new());
        assert_eq!(base_date(&with_empty).unwrap(), NaiveDate::from_ymd_opt(2026, 11, 5));
        // Day 2 already from the next run
        assert!(base_date(&run(&["2026-11-05Z", "2026-11-06Z", "2026-11-08Z"])).is_err());
        assert_eq!(base_date(&[Vec::new()]).unwrap(), None);
    }

    #[test]
    fn the_signature_changes_with_a_new_run_only() {
        let a = vec![vec![(2, date("2026-11-05")), (1, date("2026-11-05"))]];
        let same_in_another_order = vec![vec![(1, date("2026-11-05")), (2, date("2026-11-05"))]];
        let next = vec![vec![(1, date("2026-11-06")), (2, date("2026-11-06"))]];
        assert_eq!(signature(&a), signature(&same_in_another_order));
        assert_ne!(signature(&a), signature(&next));
    }

    #[test]
    fn exposure_is_counted_by_district_level_and_province() {
        let known: HashSet<String> = ["010102".to_string(), "200607".to_string()].into();
        let centers = json!({ "features": [
            { "properties": { "ubigeo": "010102", "nivel": "Nivel 3" } },
            { "properties": { "ubigeo": "010102", "nivel": "Nivel 1" } },
            { "properties": { "ubigeo": "999999", "nivel": "Nivel 2" } },
            { "properties": { "ubigeo": "010102", "nivel": null } },
        ]});
        let schools = json!({ "features": [ { "properties": { "iddistrito": 200607, "nivel": "Nivel 1" } } ] });
        let roads = json!({ "features": [
            { "properties": { "codprov": "2004", "longitud": 6951.9, "nivel": "Nivel 1" } },
            { "properties": { "codprov": "2004", "longitud": 1000.0, "nivel": "Nivel 1" } },
        ]});
        let national = json!({ "features": [ { "properties": { "coddep": "22", "longitud": 4810.4, "nivel": "Nivel 2" } } ] });
        let mut exposure = Exposure::default();
        exposure.count_points(&centers, "ubigeo", Item::PopulatedCenter, &known);
        exposure.count_points(&schools, "iddistrito", Item::School, &known);
        exposure.count_roads(&roads, 2);
        exposure.count_roads(&national, 0);

        let asuncion = &exposure.districts["010102"];
        assert_eq!(asuncion.centros_poblados, [1, 0, 1]);
        assert_eq!(asuncion.highest_level(), 3);
        assert_eq!(exposure.districts["200607"].instituciones_educativas, [1, 0, 0]);
        assert_eq!(exposure.districts["200607"].highest_level(), 1);
        // The unknown district counts for the country, not for a district
        assert_eq!(exposure.centros_poblados, [1, 1, 1]);
        assert!(exposure.unknown.contains("999999"));
        assert_eq!(kilometres(&exposure.province_roads["2004"][1]), json!([8.0, 0.0, 0.0]));
        assert_eq!(kilometres(&exposure.department_roads["22"]), json!([0.0, 4.8, 0.0]));
    }

    #[test]
    fn areas_are_read_as_polygons_and_lightened() {
        let layer = json!({ "features": [{
            "properties": { "nivel": "Nivel 2", "fecha": "2026-11-05Z" },
            "geometry": { "type": "MultiPolygon", "coordinates": [
                [[[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0], [0.0, 0.0]]],
                [[[2.0, 0.0], [3.0, 0.0], [3.0, 1.0], [2.0, 1.0], [2.0, 0.0]]],
            ]},
        }]});
        let areas: Areas = serde_json::from_value(layer).unwrap();
        let area = areas.features.into_iter().next().unwrap();
        let polygons = area.geometry.unwrap().polygons();
        assert_eq!(polygons.len(), 2);
        let map =
            json!({ "features": [{ "properties": { "nivel": 2 }, "geometry": geometry::lighten_polygons(&polygons, TOLERANCE, SCALE) }] });
        assert_eq!(properties(&map)[0]["nivel"], 2);
        assert_eq!(map["features"][0]["geometry"]["coordinates"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn codes_get_their_leading_zeros() {
        assert_eq!(code(&json!("2004"), 4).as_deref(), Some("2004"));
        assert_eq!(code(&json!(6), 2).as_deref(), Some("06"));
        assert_eq!(code(&json!(10102), 6).as_deref(), Some("010102"));
        assert_eq!(code(&json!("lima"), 6), None);
    }
}
