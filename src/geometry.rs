// SPDX-License-Identifier: AGPL-3.0-or-later
//! Polygon helpers for SENAMHI warning areas: simplification for the map and WKT for spatial queries.

use serde_json::{Value, json};

pub type Point = [f64; 2];
pub type Ring = Vec<Point>;
pub type Polygon = Vec<Ring>;

/// Tolerance (degrees) for the map: about 110 m, which is not visible at department scale.
const DISPLAY_TOLERANCE: f64 = 0.001;
/// Tolerance (degrees) for the spatial query to SENAMHI: about 1 km, enough to find the districts.
const QUERY_TOLERANCE: f64 = 0.01;
/// Coordinates for the map keep 4 decimals (about 11 m).
const DISPLAY_SCALE: f64 = 1e4;

/// The polygons of a GeoJSON `Polygon` or `MultiPolygon`; any other geometry has none.
pub fn polygons_of(geometry: &Value) -> Vec<Polygon> {
    let parse_polygon = |polygon: &Value| -> Option<Polygon> { polygon.as_array()?.iter().map(parse_ring).collect() };
    match geometry.get("type").and_then(Value::as_str) {
        Some("Polygon") => geometry.get("coordinates").and_then(parse_polygon).into_iter().collect(),
        Some("MultiPolygon") => geometry
            .get("coordinates")
            .and_then(Value::as_array)
            .map(|polygons| polygons.iter().filter_map(parse_polygon).collect())
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn parse_ring(ring: &Value) -> Option<Ring> {
    ring.as_array()?
        .iter()
        .map(|point| {
            let point = point.as_array()?;
            Some([point.first()?.as_f64()?, point.get(1)?.as_f64()?])
        })
        .collect()
}

/// Douglas-Peucker without recursion (some rings have thousands of points). Keeps both ends.
pub fn douglas_peucker(points: &[Point], tolerance: f64) -> Ring {
    if points.len() <= 2 {
        return points.to_vec();
    }
    let mut keep = vec![false; points.len()];
    keep[0] = true;
    keep[points.len() - 1] = true;
    let mut stack = vec![(0usize, points.len() - 1)];
    while let Some((a, b)) = stack.pop() {
        if b - a < 2 {
            continue;
        }
        let [x1, y1] = points[a];
        let [x2, y2] = points[b];
        let (dx, dy) = (x2 - x1, y2 - y1);
        let length_squared = dx * dx + dy * dy;
        let mut farthest = None;
        let mut farthest_distance = tolerance;
        for (i, &[x, y]) in points.iter().enumerate().take(b).skip(a + 1) {
            let distance = if length_squared == 0.0 {
                ((x - x1).powi(2) + (y - y1).powi(2)).sqrt()
            } else {
                let t = (((x - x1) * dx + (y - y1) * dy) / length_squared).clamp(0.0, 1.0);
                ((x - (x1 + t * dx)).powi(2) + (y - (y1 + t * dy)).powi(2)).sqrt()
            };
            if distance > farthest_distance {
                farthest_distance = distance;
                farthest = Some(i);
            }
        }
        if let Some(i) = farthest {
            keep[i] = true;
            stack.push((a, i));
            stack.push((i, b));
        }
    }
    points.iter().zip(keep).filter_map(|(point, kept)| kept.then_some(*point)).collect()
}

/// The warning area for the map: less detail, 4 decimals, and only rings that are still polygons.
/// Polygons whose outer ring collapses are dropped, and so are collapsed holes.
pub fn lighten(geometry: &Value) -> Value {
    let round = |value: f64| (value * DISPLAY_SCALE).round() / DISPLAY_SCALE;
    let polygons: Vec<Value> = polygons_of(geometry)
        .iter()
        .filter_map(|polygon| {
            let mut rings = polygon.iter().map(|ring| {
                douglas_peucker(ring, DISPLAY_TOLERANCE)
                    .into_iter()
                    .map(|[x, y]| [round(x), round(y)])
                    .collect::<Ring>()
            });
            let outer = rings.next().filter(|ring| ring.len() >= 4)?;
            let mut kept = vec![outer];
            kept.extend(rings.filter(|ring| ring.len() >= 4));
            Some(json!(kept))
        })
        .collect();
    json!({ "type": "MultiPolygon", "coordinates": polygons })
}

/// The area as WKT for a `cql_filter` spatial query, simplified so the request stays small.
/// A ring that would collapse keeps its original points, so no part of the area is lost.
pub fn query_wkt(geometry: &Value) -> Option<String> {
    let parts: Vec<String> = polygons_of(geometry)
        .iter()
        .filter_map(|polygon| {
            let rings: Vec<String> = polygon
                .iter()
                .filter(|ring| ring.len() >= 4)
                .map(|ring| {
                    let simplified = douglas_peucker(ring, QUERY_TOLERANCE);
                    let ring = if simplified.len() >= 4 { &simplified } else { ring };
                    let points: Vec<String> = ring.iter().map(|[x, y]| format!("{x} {y}")).collect();
                    format!("({})", points.join(","))
                })
                .collect();
            (!rings.is_empty()).then(|| format!("({})", rings.join(",")))
        })
        .collect();
    (!parts.is_empty()).then(|| format!("MULTIPOLYGON({})", parts.join(",")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square() -> Value {
        json!({ "type": "Polygon", "coordinates": [[[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0], [0.0, 0.0]]] })
    }

    #[test]
    fn douglas_peucker_drops_points_on_a_straight_line() {
        let line = [[0.0, 0.0], [0.5, 0.0000001], [1.0, 0.0]];
        assert_eq!(douglas_peucker(&line, 0.001), vec![[0.0, 0.0], [1.0, 0.0]]);
    }

    #[test]
    fn douglas_peucker_keeps_corners() {
        let path = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]];
        assert_eq!(douglas_peucker(&path, 0.001).len(), 3);
    }

    #[test]
    fn polygon_and_multipolygon_are_read() {
        assert_eq!(polygons_of(&square()).len(), 1);
        let multi = json!({ "type": "MultiPolygon", "coordinates": [square()["coordinates"], square()["coordinates"]] });
        assert_eq!(polygons_of(&multi).len(), 2);
        assert!(polygons_of(&json!({ "type": "Point", "coordinates": [0.0, 0.0] })).is_empty());
    }

    #[test]
    fn lighten_keeps_a_square_and_drops_a_collapsed_polygon() {
        let tiny =
            json!({ "type": "Polygon", "coordinates": [[[0.0, 0.0], [0.00001, 0.0], [0.00001, 0.00001], [0.0, 0.00001], [0.0, 0.0]]] });
        assert_eq!(lighten(&square())["coordinates"].as_array().unwrap().len(), 1);
        assert_eq!(lighten(&tiny)["coordinates"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn query_wkt_is_a_multipolygon() {
        assert_eq!(query_wkt(&square()).unwrap(), "MULTIPOLYGON(((0 0,1 0,1 1,0 1,0 0)))");
        assert_eq!(query_wkt(&json!({ "type": "Point", "coordinates": [0.0, 0.0] })), None);
    }
}
