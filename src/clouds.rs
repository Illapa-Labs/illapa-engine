// SPDX-License-Identifier: AGPL-3.0-or-later
//! Clouds over Peru from NOAA's GOES-19 satellite: the infrared band 13 (10.3 µm, the "clean longwave window"),
//! which gives the temperature of the top of the clouds every 10 minutes, day and night. The colder the top, the
//! taller the cloud: storms show up as patches below about −40 °C.
//!
//! Every 10 minutes the newest full-disk scan (`ABI-L2-CMIPF`, about 20 MB) is read from NOAA's public bucket on AWS
//! (open data, no account, no cost), cut to Peru, moved to the map's projection and coloured into a PNG. The images of
//! the last 12 hours are kept in memory, one every 30 minutes plus the newest one, and served by `/v1/nubes`. They are
//! not saved: after a restart the last 12 hours are read again.
//!
//! GOES data from NOAA is in the public domain.

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Datelike, Duration as Span, NaiveDate, Timelike, Utc};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use tokio::sync::RwLock;

const BUCKET: &str = "https://noaa-goes19.s3.amazonaws.com";
/// Cloud and Moisture Imagery, full disk; band 13 in the file name
const PRODUCT: &str = "ABI-L2-CMIPF";
const BAND: &str = "C13_G19_s";
/// How far back the images go, and how often one is kept (the newest is always kept too)
const KEEP_HOURS: i64 = 12;
const SLOT_MINUTES: u32 = 30;
/// How often the bucket is asked for a newer scan (the satellite scans the full disk every 10 minutes)
pub const EVERY: Duration = Duration::from_secs(600);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(180);

/// The images cover the same rectangle as the forecast images of the website: west, south, east, north
pub const BOUNDS: [f64; 4] = [-82.0, -19.0, -68.0, 1.0];
/// Image width in pixels; rows go in Mercator, like the map, so the image can be laid on it as it is
const WIDTH: usize = 768;

/// The PNG of each kept scan, by the time its scan started
pub type Frames = Arc<RwLock<BTreeMap<DateTime<Utc>, Arc<Vec<u8>>>>>;

/// When a scan started, from its file name: `..._s20263080540213_...` is 2026, day 308, 05:40:21.3 UTC.
pub fn scan_start(key: &str) -> Option<DateTime<Utc>> {
    let start = key.find(BAND)? + BAND.len();
    let digits = key.get(start..start + 13)?;
    if !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let number = |range: std::ops::Range<usize>| digits[range].parse::<u32>().ok();
    let day = NaiveDate::from_yo_opt(number(0..4)? as i32, number(4..7)?)?;
    let time = day.and_hms_opt(number(7..9)?, number(9..11)?, number(11..13)?)?;
    Some(time.and_utc())
}

/// The scans to keep: one per 30-minute slot in the last 12 hours (the scan that starts in the slot's first ten
/// minutes), plus the newest one.
pub fn wanted(scans: &BTreeMap<DateTime<Utc>, String>, now: DateTime<Utc>) -> BTreeMap<DateTime<Utc>, String> {
    let since = now - Span::hours(KEEP_HOURS);
    let mut keep: BTreeMap<_, _> = scans
        .iter()
        .filter(|(time, _)| **time >= since && time.minute() % SLOT_MINUTES < 10)
        .map(|(time, key)| (*time, key.clone()))
        .collect();
    if let Some((time, key)) = scans.iter().next_back() {
        keep.insert(*time, key.clone());
    }
    keep
}

/// The full-disk band 13 scans of one hour in the bucket, by start time.
async fn list_hour(http: &reqwest::Client, hour: DateTime<Utc>) -> Result<BTreeMap<DateTime<Utc>, String>> {
    let prefix = format!("{PRODUCT}/{}/{:03}/{:02}/", hour.year(), hour.ordinal(), hour.hour());
    let body = http
        .get(format!("{BUCKET}/?list-type=2&prefix={prefix}"))
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .context("GOES bucket")?
        .error_for_status()
        .context("GOES bucket")?
        .text()
        .await?;
    Ok(body
        .split("<Key>")
        .skip(1)
        .filter_map(|rest| rest.split("</Key>").next())
        .filter(|key| key.contains(BAND))
        .filter_map(|key| Some((scan_start(key)?, key.to_string())))
        .collect())
}

/// Asks the bucket for the scans of the last 12 hours, draws the ones that are missing (the newest first, so it
/// shows up soon after a restart) and forgets the ones no longer wanted.
pub async fn update(http: &reqwest::Client, frames: &Frames) -> Result<()> {
    let now = Utc::now();
    let mut scans = BTreeMap::new();
    for hours_ago in 0..=KEEP_HOURS {
        scans.extend(list_hour(http, now - Span::hours(hours_ago)).await?);
    }
    let wanted = wanted(&scans, now);
    frames.write().await.retain(|time, _| wanted.contains_key(time));
    for (time, key) in wanted.iter().rev() {
        if frames.read().await.contains_key(time) {
            continue;
        }
        let bytes = http
            .get(format!("{BUCKET}/{key}"))
            .timeout(DOWNLOAD_TIMEOUT)
            .send()
            .await
            .context("GOES scan")?
            .error_for_status()
            .context("GOES scan")?
            .bytes()
            .await?;
        let png = tokio::task::spawn_blocking(move || draw(&bytes)).await??;
        let kb = png.len() / 1024;
        frames.write().await.insert(*time, Arc::new(png));
        tracing::info!(scan = %time, kb, "GOES-19 clouds drawn");
    }
    Ok(())
}

/// Keeps the images up to date, every 10 minutes. A failure (the bucket, a bad file) is retried the next time.
pub async fn run(http: reqwest::Client, frames: Frames) {
    loop {
        if let Err(error) = update(&http, &frames).await {
            tracing::warn!("GOES-19 clouds not updated, the previous images stay: {error:#}");
        }
        tokio::time::sleep(EVERY).await;
    }
}

/// The satellite's view, from the file: where the scan angles start and how far apart they are, and the projection.
struct View {
    x0: f64,
    dx: f64,
    y0: f64,
    dy: f64,
    columns: usize,
    a: f64,
    b: f64,
    /// Distance from the centre of the Earth to the satellite
    h: f64,
    longitude: f64,
}

impl View {
    /// The scan angles (x, y, in radians) at which the satellite sees a place (GOES-R product user guide, 4.2.8.1).
    fn angles(&self, latitude: f64, longitude: f64) -> Option<(f64, f64)> {
        let (phi, lambda) = (latitude.to_radians(), longitude.to_radians());
        let phi_c = ((self.b * self.b) / (self.a * self.a) * phi.tan()).atan();
        let e2 = 1.0 - (self.b * self.b) / (self.a * self.a);
        let rc = self.b / (1.0 - e2 * phi_c.cos().powi(2)).sqrt();
        let sx = self.h - rc * phi_c.cos() * (lambda - self.longitude).cos();
        let sy = -rc * phi_c.cos() * (lambda - self.longitude).sin();
        let sz = rc * phi_c.sin();
        // Behind the horizon: the satellite does not see it
        if self.h * (self.h - sx) < sy * sy + (self.a * self.a) / (self.b * self.b) * sz * sz {
            return None;
        }
        Some(((-sy / (sx * sx + sy * sy + sz * sz).sqrt()).asin(), (sz / sx).atan()))
    }

    /// The pixel (row, column) of the scan that sees a place
    fn pixel(&self, latitude: f64, longitude: f64) -> Option<(usize, usize)> {
        let (x, y) = self.angles(latitude, longitude)?;
        let column = ((x - self.x0) / self.dx).round();
        let row = ((y - self.y0) / self.dy).round();
        (column >= 0.0 && row >= 0.0 && (column as usize) < self.columns).then_some((row as usize, column as usize))
    }
}

fn mercator(latitude: f64) -> f64 {
    (std::f64::consts::FRAC_PI_4 + latitude.to_radians() / 2.0).tan().ln()
}

/// The image height for `WIDTH`, so that pixels are square in Mercator
fn height() -> usize {
    let [west, south, east, north] = BOUNDS;
    ((WIDTH as f64) * (mercator(north) - mercator(south)) / (east - west).to_radians()).round() as usize
}

/// The place at the centre of each image pixel: latitudes by row (in Mercator steps) and longitudes by column
fn pixel_places() -> (Vec<f64>, Vec<f64>) {
    let [west, south, east, north] = BOUNDS;
    let rows = height();
    let latitudes = (0..rows)
        .map(|y| {
            let m = mercator(north) - (y as f64 + 0.5) / rows as f64 * (mercator(north) - mercator(south));
            (2.0 * m.exp().atan() - std::f64::consts::FRAC_PI_2).to_degrees()
        })
        .collect();
    let longitudes = (0..WIDTH).map(|x| west + (x as f64 + 0.5) / WIDTH as f64 * (east - west)).collect();
    (latitudes, longitudes)
}

/// The colour of a cloud top by its temperature (°C): transparent for clear sky or warm clouds, greys for clouds, and
/// cold colours for storm tops. No yellow, orange or red: those are the colours of SENAMHI's warning on the map.
const STOPS: [(f64, [u8; 4]); 8] = [
    (-85.0, [255, 255, 255, 255]),
    (-75.0, [190, 40, 200, 245]),
    (-65.0, [90, 40, 190, 235]),
    (-55.0, [40, 120, 220, 225]),
    (-45.0, [90, 200, 225, 210]),
    (-35.0, [150, 158, 168, 170]),
    (-20.0, [190, 196, 204, 110]),
    (-10.0, [200, 205, 212, 0]),
];

/// The palette: one colour per degree, from the warmest that shows (-10 °C) to the coldest (-85 °C)
const WARMEST: f64 = -10.0;
const LEVELS: usize = 75;

/// The palette index of a temperature: 0 (transparent) when warmer than -10 °C, then one per degree
pub fn level(celsius: f64) -> u8 {
    if !celsius.is_finite() || celsius >= WARMEST {
        return 0;
    }
    1 + ((WARMEST - celsius).floor() as usize).min(LEVELS - 1) as u8
}

pub fn colour(celsius: f64) -> [u8; 4] {
    if !celsius.is_finite() || celsius >= STOPS[STOPS.len() - 1].0 {
        return [0, 0, 0, 0];
    }
    if celsius <= STOPS[0].0 {
        return STOPS[0].1;
    }
    let upper = STOPS.iter().position(|(at, _)| *at >= celsius).unwrap_or(STOPS.len() - 1);
    let ((t0, c0), (t1, c1)) = (STOPS[upper - 1], STOPS[upper]);
    let f = (celsius - t0) / (t1 - t0);
    std::array::from_fn(|k| (c0[k] as f64 + (c1[k] as f64 - c0[k] as f64) * f).round() as u8)
}

fn float_attr(attrs: &std::collections::HashMap<String, hdf5_pure::AttrValue>, name: &str) -> Result<f64> {
    use hdf5_pure::AttrValue::*;
    Ok(match attrs.get(name).with_context(|| format!("attribute {name}"))? {
        F32(v) => f64::from(*v),
        F64(v) => *v,
        F32Array(v) => f64::from(*v.first().context(name.to_string())?),
        F64Array(v) => *v.first().context(name.to_string())?,
        other => bail!("attribute {name}: unexpected {other:?}"),
    })
}

/// A GOES-19 band 13 scan (NetCDF 4) to a PNG of Peru, coloured by the temperature of the cloud tops.
pub fn draw(netcdf: &[u8]) -> Result<Vec<u8>> {
    let file = hdf5_pure::File::from_bytes(netcdf.to_vec()).context("GOES scan is not a readable NetCDF 4 file")?;
    let projection = file.dataset("goes_imager_projection")?.attrs()?;
    let (a, b) = (
        float_attr(&projection, "semi_major_axis")?,
        float_attr(&projection, "semi_minor_axis")?,
    );
    let axis = |name: &str| -> Result<(f64, f64, usize)> {
        let dataset = file.dataset(name)?;
        let attrs = dataset.attrs()?;
        let values = dataset.read_i16()?;
        let (scale, offset) = (float_attr(&attrs, "scale_factor")?, float_attr(&attrs, "add_offset")?);
        Ok((
            f64::from(values[0]) * scale + offset,
            scale * f64::from(values[1] - values[0]),
            values.len(),
        ))
    };
    let ((x0, dx, columns), (y0, dy, _)) = (axis("x")?, axis("y")?);
    let view = View {
        x0,
        dx,
        y0,
        dy,
        columns,
        a,
        b,
        h: float_attr(&projection, "perspective_point_height")? + a,
        longitude: float_attr(&projection, "longitude_of_projection_origin")?.to_radians(),
    };

    // Which scan pixel each image pixel takes, and the band of scan rows that covers Peru (read only those)
    let (latitudes, longitudes) = pixel_places();
    let pixels: Vec<Option<(usize, usize)>> = latitudes
        .iter()
        .flat_map(|lat| longitudes.iter().map(|lon| view.pixel(*lat, *lon)))
        .collect();
    let first = pixels
        .iter()
        .flatten()
        .map(|(row, _)| *row)
        .min()
        .context("Peru is not in the scan")?;
    let last = pixels.iter().flatten().map(|(row, _)| *row).max().unwrap_or(first);
    let cmi = file.dataset("CMI")?;
    let attrs = cmi.attrs()?;
    let (scale, offset) = (float_attr(&attrs, "scale_factor")?, float_attr(&attrs, "add_offset")?);
    let band = cmi.read_u16_rows(first as u64, (last - first + 1) as u64)?;

    // One palette entry per degree from -10 to -85 °C (index 0 is transparent): a PNG with a palette weighs a
    // fraction of one with full colour, which matters on a phone
    let palette: Vec<[u8; 4]> = std::iter::once([0, 0, 0, 0])
        .chain((0..LEVELS).map(|k| colour(WARMEST - k as f64 - 0.5)))
        .collect();
    let mut indices = Vec::with_capacity(pixels.len());
    for pixel in &pixels {
        let raw = pixel.and_then(|(row, column)| band.get((row - first) * columns + column).copied());
        // 65535 is the fill value (-1 stored as unsigned): no data
        let index = match raw {
            Some(value) if value != u16::MAX => level(f64::from(value) * scale + offset - 273.15),
            _ => 0,
        };
        indices.push(index);
    }

    let mut png = Vec::new();
    let mut encoder = png::Encoder::new(&mut png, WIDTH as u32, height() as u32);
    encoder.set_color(png::ColorType::Indexed);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_palette(palette.iter().flat_map(|c| [c[0], c[1], c[2]]).collect::<Vec<u8>>());
    encoder.set_trns(palette.iter().map(|c| c[3]).collect::<Vec<u8>>());
    encoder.set_compression(png::Compression::High);
    encoder.write_header()?.write_image_data(&indices)?;
    Ok(png)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scan_start_comes_from_the_file_name() {
        let key = "ABI-L2-CMIPF/2026/308/05/OR_ABI-L2-CMIPF-M6C13_G19_s20263080540213_e20263080549533_c20263080549592.nc";
        assert_eq!(scan_start(key).unwrap().to_rfc3339(), "2026-11-04T05:40:21+00:00");
        assert!(scan_start("OR_ABI-L2-CMIPF-M6C02_G19_s20263080540213.nc").is_none());
    }

    #[test]
    fn it_keeps_one_scan_per_half_hour_and_the_newest() {
        let now = "2026-11-04T06:05:00Z".parse::<DateTime<Utc>>().unwrap();
        let scans: BTreeMap<_, _> = (0..80)
            .map(|k| (now - Span::minutes(5) - Span::minutes(10 * k), format!("scan {k}")))
            .map(|(time, key)| (time.with_second(21).unwrap(), key))
            .collect();
        let keep = wanted(&scans, now);
        let newest = scans.keys().next_back().unwrap();
        assert!(keep.contains_key(newest));
        assert!(keep.keys().all(|time| *time >= now - Span::hours(12)));
        assert!(keep.keys().filter(|time| *time != newest).all(|time| time.minute() % 30 < 10));
        // 18:30 to 06:00, every 30 minutes
        assert_eq!(keep.len(), 24);
    }

    #[test]
    fn the_satellite_sees_the_point_under_it_at_the_centre() {
        let view = View {
            x0: -0.151844,
            dx: 5.6e-5,
            y0: 0.151844,
            dy: -5.6e-5,
            columns: 5424,
            a: 6378137.0,
            b: 6356752.31414,
            h: 35786023.0 + 6378137.0,
            longitude: (-75.0f64).to_radians(),
        };
        let (row, column) = view.pixel(0.0, -75.0).unwrap();
        assert!((row as i64 - 2711).abs() <= 1 && (column as i64 - 2711).abs() <= 1);
        // Lima is south (lower in the scan) and a little west of the sub-satellite point
        let (row, column) = view.pixel(-12.05, -77.04).unwrap();
        assert!(row > 2711 && column < 2711);
        // The other side of the Earth is not seen
        assert!(view.pixel(0.0, 105.0).is_none());
    }

    #[test]
    fn warm_clouds_are_transparent_and_storm_tops_are_coloured() {
        assert_eq!(colour(15.0)[3], 0);
        assert_eq!(colour(f64::NAN)[3], 0);
        assert!(colour(-30.0)[3] > 100);
        let storm = colour(-60.0);
        assert!(storm[2] > storm[0], "storm tops are blue or violet, never red");
        assert_eq!(colour(-90.0), [255, 255, 255, 255]);
    }

    #[test]
    fn each_degree_has_its_palette_entry() {
        assert_eq!(level(20.0), 0);
        assert_eq!(level(-10.5), 1);
        assert_eq!(level(-84.5), 75);
        assert_eq!(level(-100.0), 75);
    }
}
