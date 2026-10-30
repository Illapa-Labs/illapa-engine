// SPDX-License-Identifier: AGPL-3.0-or-later
//! The lists of departments, provinces and districts with their warnings. They are built once after every refresh
//! and served from memory, so requests never query the database.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use std::collections::HashMap;

use crate::senamhi::fold;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Place {
    pub ubigeo: String,
    pub nombre: String,
}

/// Populated centers or people, by warning level.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct ByLevel {
    pub amarillo: i32,
    pub naranja: i32,
    pub rojo: i32,
}

impl ByLevel {
    fn total(&self) -> i64 {
        self.amarillo as i64 + self.naranja as i64 + self.rojo as i64
    }
}

/// SENAMHI levels: 0 no warning, 2 yellow, 3 orange, 4 red.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct Levels {
    pub nivel: i32,
    pub color: Option<&'static str>,
    pub nivel_lluvia: i32,
    pub nivel_quebradas: i32,
}

impl Levels {
    fn new(rain: i32, ravine: i32) -> Self {
        let level = rain.max(ravine);
        Self {
            nivel: level,
            color: color(level),
            nivel_lluvia: rain,
            nivel_quebradas: ravine,
        }
    }
}

fn color(level: i32) -> Option<&'static str> {
    match level {
        2 => Some("amarillo"),
        3 => Some("naranja"),
        4 => Some("rojo"),
        _ => None,
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct District {
    pub ubigeo: String,
    pub nombre: String,
    pub provincia: Place,
    pub departamento: Place,
    /// Counted in the 2017 census
    pub poblacion: i32,
    /// Counted plus INEI's estimate of people the census missed
    pub poblacion_total: i32,
    pub centros_poblados_en_zona: ByLevel,
    pub personas_en_zona: ByLevel,
    #[serde(flatten)]
    pub levels: Levels,
}

#[derive(Clone, Debug, Serialize)]
pub struct Province {
    pub ubigeo: String,
    pub nombre: String,
    pub departamento: Place,
    pub poblacion: i32,
    pub poblacion_total: i32,
    pub distritos_con_aviso: i32,
    pub personas_en_zona: i64,
    #[serde(flatten)]
    pub levels: Levels,
}

#[derive(Clone, Debug, Serialize)]
pub struct Department {
    pub ubigeo: String,
    pub nombre: String,
    pub poblacion: i32,
    pub poblacion_total: i32,
    pub nivel: i32,
    pub color: Option<&'static str>,
    pub distritos_con_aviso: i32,
    pub personas_en_zona: i64,
}

#[derive(Default)]
pub struct Snapshot {
    pub departments: Vec<Department>,
    pub provinces: Vec<Province>,
    pub districts: Vec<District>,
    /// When SENAMHI data was last downloaded (RFC 3339)
    pub actualizado: Option<String>,
    /// Date of the ravine activation warning
    pub fecha_quebradas: Option<String>,
}

/// Rows as they come from the database, before the levels are added up.
pub struct RawPlace {
    pub ubigeo: String,
    pub nombre: String,
    pub poblacion: i32,
    pub poblacion_total: i32,
}

pub struct RawDistrict {
    pub place: RawPlace,
    pub rain_level: i32,
    pub ravine_level: i32,
    pub populated_centers: ByLevel,
    pub people: ByLevel,
}

#[derive(Deserialize, Default)]
pub struct Filters {
    /// Department ubigeo (2 digits) or name, for example `15` or `lima`
    pub departamento: Option<String>,
    /// Province ubigeo (4 digits) or name
    pub provincia: Option<String>,
    /// Only those with a warning of this level or higher (2, 3 or 4)
    pub nivel_minimo: Option<i32>,
    /// `true`: only those with some warning
    pub solo_avisos: Option<bool>,
    /// Maximum number of rows
    pub limite: Option<usize>,
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

    fn limit(&self, maximum: usize) -> usize {
        self.limite.unwrap_or(maximum).clamp(1, maximum)
    }
}

/// A place given by ubigeo or by name, without caring about case or accents.
fn matches(place: &Place, wanted: &str) -> bool {
    place.ubigeo == wanted.trim() || fold(&place.nombre) == fold(wanted)
}

impl Snapshot {
    /// Reads everything from the database and builds the lists.
    pub async fn load(pool: &PgPool) -> Result<Self> {
        let place = |row: &sqlx::postgres::PgRow| RawPlace {
            ubigeo: row.get::<String, _>("ubigeo").trim().to_string(),
            nombre: row.get("nombre"),
            poblacion: row.get("poblacion_censada"),
            poblacion_total: row.get("poblacion_total"),
        };
        let departments: Vec<RawPlace> = sqlx::query("select ubigeo, nombre, poblacion_censada, poblacion_total from departamentos")
            .fetch_all(pool)
            .await
            .context("departments")?
            .iter()
            .map(place)
            .collect();
        let provinces: Vec<RawPlace> = sqlx::query("select ubigeo, nombre, poblacion_censada, poblacion_total from provincias")
            .fetch_all(pool)
            .await
            .context("provinces")?
            .iter()
            .map(place)
            .collect();
        let districts: Vec<RawDistrict> = sqlx::query(
            "select x.ubigeo, x.nombre, x.poblacion_censada, x.poblacion_total,
                    coalesce(a.nivel_lluvia, 0)::int as nivel_lluvia, coalesce(a.nivel_quebradas, 0)::int as nivel_quebradas,
                    coalesce(a.centros_poblados_amarillo, 0) as centros_amarillo,
                    coalesce(a.centros_poblados_naranja, 0) as centros_naranja,
                    coalesce(a.centros_poblados_rojo, 0) as centros_rojo,
                    coalesce(a.personas_amarillo, 0) as personas_amarillo,
                    coalesce(a.personas_naranja, 0) as personas_naranja,
                    coalesce(a.personas_rojo, 0) as personas_rojo
               from distritos x left join distrito_aviso a on a.ubigeo = x.ubigeo",
        )
        .fetch_all(pool)
        .await
        .context("districts")?
        .iter()
        .map(|row| RawDistrict {
            place: place(row),
            rain_level: row.get("nivel_lluvia"),
            ravine_level: row.get("nivel_quebradas"),
            populated_centers: ByLevel {
                amarillo: row.get("centros_amarillo"),
                naranja: row.get("centros_naranja"),
                rojo: row.get("centros_rojo"),
            },
            people: ByLevel {
                amarillo: row.get("personas_amarillo"),
                naranja: row.get("personas_naranja"),
                rojo: row.get("personas_rojo"),
            },
        })
        .collect();
        let refresh = sqlx::query("select actualizado, fecha_quebradas from refresco where id = 1")
            .fetch_optional(pool)
            .await
            .context("last refresh")?;
        let (actualizado, fecha_quebradas) = match refresh {
            Some(row) => (
                Some(row.get::<chrono::DateTime<chrono::Utc>, _>("actualizado").to_rfc3339()),
                row.get::<Option<String>, _>("fecha_quebradas"),
            ),
            None => (None, None),
        };
        Ok(Self::assemble(departments, provinces, districts, actualizado, fecha_quebradas))
    }

    /// Adds up the levels of each province and department from its districts, and sorts every list from the
    /// highest warning down.
    pub fn assemble(
        departments: Vec<RawPlace>,
        provinces: Vec<RawPlace>,
        districts: Vec<RawDistrict>,
        actualizado: Option<String>,
        fecha_quebradas: Option<String>,
    ) -> Self {
        let department_names: HashMap<String, String> = departments.iter().map(|d| (d.ubigeo.clone(), d.nombre.clone())).collect();
        let province_names: HashMap<String, String> = provinces.iter().map(|p| (p.ubigeo.clone(), p.nombre.clone())).collect();
        let place_of = |names: &HashMap<String, String>, ubigeo: &str| Place {
            ubigeo: ubigeo.to_string(),
            nombre: names.get(ubigeo).cloned().unwrap_or_default(),
        };

        let mut districts: Vec<District> = districts
            .into_iter()
            .map(|raw| District {
                provincia: place_of(&province_names, &raw.place.ubigeo[..4]),
                departamento: place_of(&department_names, &raw.place.ubigeo[..2]),
                ubigeo: raw.place.ubigeo,
                nombre: raw.place.nombre,
                poblacion: raw.place.poblacion,
                poblacion_total: raw.place.poblacion_total,
                centros_poblados_en_zona: raw.populated_centers,
                personas_en_zona: raw.people,
                levels: Levels::new(raw.rain_level, raw.ravine_level),
            })
            .collect();
        districts.sort_by(|a, b| {
            b.levels
                .nivel
                .cmp(&a.levels.nivel)
                .then(b.poblacion.cmp(&a.poblacion))
                .then(a.ubigeo.cmp(&b.ubigeo))
        });

        /// What a province or department gets from its districts.
        #[derive(Default)]
        struct Totals {
            rain: i32,
            ravine: i32,
            with_warning: i32,
            people: i64,
        }
        let mut by_province: HashMap<&str, Totals> = HashMap::new();
        let mut by_department: HashMap<&str, Totals> = HashMap::new();
        for district in &districts {
            for totals in [
                by_province.entry(&district.ubigeo[..4]).or_default(),
                by_department.entry(&district.ubigeo[..2]).or_default(),
            ] {
                totals.rain = totals.rain.max(district.levels.nivel_lluvia);
                totals.ravine = totals.ravine.max(district.levels.nivel_quebradas);
                totals.with_warning += i32::from(district.levels.nivel >= 2);
                totals.people += district.personas_en_zona.total();
            }
        }

        let empty = Totals::default();
        let mut provinces: Vec<Province> = provinces
            .into_iter()
            .map(|raw| {
                let totals = by_province.get(raw.ubigeo.as_str()).unwrap_or(&empty);
                Province {
                    departamento: place_of(&department_names, &raw.ubigeo[..2]),
                    poblacion: raw.poblacion,
                    poblacion_total: raw.poblacion_total,
                    distritos_con_aviso: totals.with_warning,
                    personas_en_zona: totals.people,
                    levels: Levels::new(totals.rain, totals.ravine),
                    ubigeo: raw.ubigeo,
                    nombre: raw.nombre,
                }
            })
            .collect();
        // Names are compared without accents, so «CAÑETE» goes next to «CANGALLO» and not after «Z»
        provinces.sort_by(|a, b| b.levels.nivel.cmp(&a.levels.nivel).then(fold(&a.nombre).cmp(&fold(&b.nombre))));

        let mut departments: Vec<Department> = departments
            .into_iter()
            .map(|raw| {
                let totals = by_department.get(raw.ubigeo.as_str()).unwrap_or(&empty);
                let level = totals.rain.max(totals.ravine);
                Department {
                    poblacion: raw.poblacion,
                    poblacion_total: raw.poblacion_total,
                    nivel: level,
                    color: color(level),
                    distritos_con_aviso: totals.with_warning,
                    personas_en_zona: totals.people,
                    ubigeo: raw.ubigeo,
                    nombre: raw.nombre,
                }
            })
            .collect();
        departments.sort_by(|a, b| {
            b.nivel
                .cmp(&a.nivel)
                .then(b.distritos_con_aviso.cmp(&a.distritos_con_aviso))
                .then(fold(&a.nombre).cmp(&fold(&b.nombre)))
        });

        Self {
            departments,
            provinces,
            districts,
            actualizado,
            fecha_quebradas,
        }
    }

    pub fn provinces(&self, filters: &Filters) -> Vec<&Province> {
        let minimum = filters.minimum_level();
        self.provinces
            .iter()
            .filter(|province| {
                filters
                    .departamento
                    .as_deref()
                    .is_none_or(|wanted| matches(&province.departamento, wanted))
            })
            .filter(|province| {
                filters.provincia.as_deref().is_none_or(|wanted| {
                    matches(
                        &Place {
                            ubigeo: province.ubigeo.clone(),
                            nombre: province.nombre.clone(),
                        },
                        wanted,
                    )
                })
            })
            .filter(|province| province.levels.nivel >= minimum)
            .take(filters.limit(500))
            .collect()
    }

    pub fn districts(&self, filters: &Filters) -> Vec<&District> {
        let minimum = filters.minimum_level();
        self.districts
            .iter()
            .filter(|district| {
                filters
                    .departamento
                    .as_deref()
                    .is_none_or(|wanted| matches(&district.departamento, wanted))
            })
            .filter(|district| {
                filters
                    .provincia
                    .as_deref()
                    .is_none_or(|wanted| matches(&district.provincia, wanted))
            })
            .filter(|district| district.levels.nivel >= minimum)
            .take(filters.limit(2000))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place(ubigeo: &str, nombre: &str, poblacion: i32) -> RawPlace {
        RawPlace {
            ubigeo: ubigeo.into(),
            nombre: nombre.into(),
            poblacion,
            poblacion_total: poblacion,
        }
    }

    fn district(ubigeo: &str, nombre: &str, poblacion: i32, rain: i32, ravine: i32, people: ByLevel) -> RawDistrict {
        RawDistrict {
            place: place(ubigeo, nombre, poblacion),
            rain_level: rain,
            ravine_level: ravine,
            populated_centers: ByLevel::default(),
            people,
        }
    }

    fn sample() -> Snapshot {
        Snapshot::assemble(
            vec![place("15", "LIMA", 1000), place("20", "PIURA", 500)],
            vec![
                place("1501", "LIMA", 800),
                place("1507", "HUAROCHIRI", 200),
                place("2001", "PIURA", 500),
            ],
            vec![
                district("150101", "LIMA", 300, 0, 0, ByLevel::default()),
                district(
                    "150702",
                    "ANTIOQUIA",
                    50,
                    2,
                    3,
                    ByLevel {
                        amarillo: 0,
                        naranja: 40,
                        rojo: 0,
                    },
                ),
                district("200101", "PIURA", 400, 4, 0, ByLevel::default()),
                district(
                    "200104",
                    "CASTILLA",
                    100,
                    0,
                    2,
                    ByLevel {
                        amarillo: 10,
                        naranja: 0,
                        rojo: 0,
                    },
                ),
            ],
            Some("2026-10-30T00:00:00+00:00".into()),
            None,
        )
    }

    #[test]
    fn levels_add_up_to_provinces_and_departments() {
        let snapshot = sample();
        let piura = &snapshot.departments[0];
        assert_eq!((piura.nombre.as_str(), piura.nivel, piura.color), ("PIURA", 4, Some("rojo")));
        assert_eq!((piura.distritos_con_aviso, piura.personas_en_zona), (2, 10));
        let huarochiri = snapshot.provinces.iter().find(|p| p.ubigeo == "1507").unwrap();
        assert_eq!(huarochiri.levels, Levels::new(2, 3));
        assert_eq!(huarochiri.personas_en_zona, 40);
        let lima = snapshot.provinces.iter().find(|p| p.ubigeo == "1501").unwrap();
        assert_eq!((lima.levels.nivel, lima.distritos_con_aviso), (0, 0));
    }

    #[test]
    fn districts_come_from_the_highest_warning_down() {
        let snapshot = sample();
        let names: Vec<&str> = snapshot.districts.iter().map(|d| d.nombre.as_str()).collect();
        assert_eq!(names, ["PIURA", "ANTIOQUIA", "CASTILLA", "LIMA"]);
    }

    #[test]
    fn filters_accept_ubigeo_or_name_without_accents() {
        let snapshot = sample();
        let by_name = Filters {
            provincia: Some("Huarochirí".into()),
            ..Default::default()
        };
        assert_eq!(snapshot.districts(&by_name).len(), 1);
        let only_warnings = Filters {
            departamento: Some("15".into()),
            solo_avisos: Some(true),
            ..Default::default()
        };
        assert_eq!(
            snapshot
                .districts(&only_warnings)
                .iter()
                .map(|d| d.ubigeo.as_str())
                .collect::<Vec<_>>(),
            ["150702"]
        );
        let red = Filters {
            nivel_minimo: Some(4),
            ..Default::default()
        };
        assert_eq!(snapshot.provinces(&red).len(), 1);
        let one = Filters {
            limite: Some(1),
            ..Default::default()
        };
        assert_eq!(snapshot.districts(&one).len(), 1);
    }
}
