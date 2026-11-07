[Español](README.md) | **English**

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="branding/chakana-mark-for-dark-backgrounds.svg">
    <img alt="Illapa Labs" src="branding/chakana-mark-for-light-backgrounds.svg" width="96">
  </picture>
</p>

# Illapa Engine

[![License: AGPL v3](https://img.shields.io/badge/license-AGPL%20v3-blue.svg)](LICENSE)

The API of [Illapa Labs](https://github.com/Illapa-Labs). It asks SENAMHI (Peru's national meteorology and hydrology service) for the rain warning and the ravine activation warning, works out which districts each one touches and how many people live in the warning areas, and serves it as ready-to-use JSON. The [Perú Alerta Lluvia](https://github.com/Illapa-Labs/peru-alerta-lluvia) site will consume it instead of doing that work itself.

> **This is not an official warning system.** The source of the warnings is [SENAMHI](https://www.senamhi.gob.pe). In an emergency in Peru, call 115 (INDECI) or 116 (Fire Department).

## Endpoints

| Path | Returns |
|---|---|
| `GET /v1/aviso` | The rain warning (24 h) as GeoJSON, lightened for the map. `503` until the first refresh |
| `GET /v1/departamentos` | The 25 departments with their population, highest level and how many districts have a warning |
| `GET /v1/provincias` | The provinces, with the same data |
| `GET /v1/distritos` | The districts with their rain and ravine levels, and the populated centers and people in warning areas, by level |
| `GET /v1/pronostico` | The hourly rain forecast (36 hours) on a 0.2° grid (about 22 km) over Peru and, in the `zonas` field, on 0.1° grids (the model's real resolution, about 11 km) over the areas with ravines: Lima Este, the north coast, Áncash, Cusco and Arequipa. It is the UK Met Office global 10 km model via Open-Meteo (CC BY-SA 4.0). `503` until the first forecast |
| `GET /v1/nubes` | Clouds from the GOES-19 satellite (NOAA, public domain) over the last 12 hours: one PNG every 30 minutes plus the newest (about 10 minutes behind), colouring the temperature of the cloud tops. Each image covers `oeste`, `sur`, `este`, `norte`, with rows in Mercator. `503` until the first image |
| `GET /v1/nubes/{archivo}.png` | One of those images; it never changes, so it is kept for a day |
| `GET /v1/inundaciones` | SENAMHI's river flood warnings (SONICS-PBI) for the current state and the next 7 days: what each level means, each day's date and totals, and what is exposed in each district (see [River flood warnings](#river-flood-warnings)). `503` until the first run |
| `GET /v1/inundaciones/{dia}` | One day's subbasins with a warning as GeoJSON, lightened for the map (about 340 KB, 95 KB compressed): `0` is the current state and `1` to `7` the forecast |
| `GET /healthz` | `ok` if the API is up |

`/v1/provincias` and `/v1/distritos` accept these filters:

| Filter | Example | What it does |
|---|---|---|
| `departamento` | `15` or `lima` | Only that department (ubigeo or name) |
| `provincia` | `1501` or `huarochiri` | Only that province |
| `solo_avisos` | `true` | Only those with some warning |
| `nivel_minimo` | `3` | Only those at that level or higher (2 yellow, 3 orange, 4 red) |
| `limite` | `100` | Maximum number of rows |

Example: `GET /v1/distritos?departamento=lima&solo_avisos=true`.

Field names are in Spanish, like the data they describe. Lists include `actualizado` (when the data was downloaded), `fecha_quebradas` (the date of the ravine warning) and `fuente`. Responses are compressed; how long they can be kept is in [Cache and CDN](#cache-and-cdn). Until the first refresh, the lists answer `503`: not knowing yet is not the same as having no warnings.

## How it works

Every 30 minutes, in the background:

1. It downloads SENAMHI's rain warning and asks its server, with a spatial query, which districts each area touches.
2. It downloads the ravine activation warning and the populated centers inside those areas, with their population.
3. It matches everything with INEI's districts (2017 census) and saves it in Postgres in a single transaction: if something fails, the previous data keeps being served.

Every 12 hours it asks Open-Meteo for the UK Met Office rain forecast at about 3,070 points over Peru (0.2° grid) and about 800 more in the 0.1° zones (`data/zonas-pronostico.json`, regenerated with `scripts/generar-zonas-pronostico.py`), in batches with pauses to respect its limits (it takes about 10 minutes). Every point is counted as a call, because it is not clear whether Open-Meteo does: two runs a day are about 7,700 of the 10,000 a day of its free plan, and a test checks that it is not exceeded. To add a zone, take points from another or ask for fewer runs.

Every hour it asks SENAMHI only for the levels and dates of the river flood warnings (a few hundred bytes); when a new run is out, it downloads the 8 days and what is exposed (about 100 MB in all, a minute and a half) and saves it.

On start it serves the last saved warning, forecast and river flood warnings, so there is no need to wait for the first refresh.

## River flood warnings

SENAMHI's SONICS-PBI service ("impact-based forecasts for river floods") marks the subbasins where a river may overflow or rise a lot. It is issued once a day (around 1 pm in Lima): the **current state** (day `0`) is the conditions observed up to the day before the issue, and **days 1 to 7** are the forecast from the day of the issue on. Each day has its `fecha`; `observado_hasta` is the current state's.

The levels, as SENAMHI describes them in its [Observatorio](https://observatorio.senamhi.gob.pe/investigadores/agua/sonics):

| Level | Colour | Hazard |
|---|---|---|
| 1 | Yellow | Likely conditions that could affect activities on the river and nearby areas |
| 2 | Orange | Risky conditions for activities on the river and nearby areas, with likely bank erosion and damage to infrastructure near the river |
| 3 | Red | Expected overflow and bank erosion; possible flooding in the lowest populated and farm areas |

For all three it recommends taking precautions and avoiding activities near the river. The coloured area is the subbasin where there may be an impact: the area that actually floods may vary within it.

In `/v1/inundaciones`, counts by level come as lists of three: `[level 1, level 2, level 3]`.

- `dias`: for each day, the country's `fecha`, `subcuencas` (subbasins), `centros_poblados` (populated centers), `instituciones_educativas` (schools), `establecimientos_salud` (health facilities) and `vias_km` (road km: `nacional`, `departamental`, `vecinal`), and `mapa`, the path of its GeoJSON.
- `distritos`: by ubigeo, the days when the district has something exposed, with its highest `nivel` and its populated centers, schools and health facilities by level. A district is listed only if it has one of those in a subbasin with a warning.
- `provincias` and `departamentos`: road km by level. SENAMHI does not give the district of the roads: departmental and neighbourhood roads carry their province, national roads their department.
- `niveles`: each level's name, colour, hazard and recommendation (in Spanish, SENAMHI's words).

## Cache and CDN

The data does not change with every visit: the warning every half hour and the forecast every 12 hours. So the API builds the lists in memory after each refresh (no request queries the database or SENAMHI) and tells browsers and CDNs how long they may keep each response:

| Data | Browser (`Cache-Control`) | CDN (`CDN-Cache-Control`) |
|---|---|---|
| Warning, departments, provinces, districts | 5 min, then 25 more min serving the previous copy while fetching the new one | 10 min, then 20 more min serving the previous one |
| Forecast | 30 min, then 1 more hour serving the previous one | same |
| River flood warnings | 30 min, then 1 more hour serving the previous one | same |
| Errors (`503`) and `/healthz` | not kept (`no-store`) | not kept |

Every data response also allows serving the last copy for up to a day if the API fails (`stale-if-error`): a warning from a while ago is better than an empty page. There is no `s-maxage`, because it stops a CDN from serving the previous copy while it fetches the new one.

### Cloudflare rule (when deploying)

With the API behind Cloudflare, thousands of visits turn into a few requests to the API every 10 minutes. But **Cloudflare does not cache JSON responses by default**: a rule is needed.

1. The DNS record for `api.illapalabs.org` must be **proxied** (orange cloud).
2. In *Caching → Cache Rules*, create a rule for `api.illapalabs.org` whose path starts with `/v1/`, set to **Eligible for cache** and respecting the origin headers (*Edge TTL* and *Browser TTL*: use the origin's `Cache-Control`).
3. Do not turn on *Always Online* for this path: with it, Cloudflare ignores `stale-while-revalidate` and `stale-if-error`.
4. Check: `curl -sI https://api.illapalabs.org/v1/aviso | grep -i cf-cache-status` should say `HIT` from the second request on (`MISS` the first time, `UPDATING` while it renews).

The option names in the Cloudflare dashboard change over time: if they do not match, see [Cache Rules](https://developers.cloudflare.com/cache/how-to/cache-rules/settings/) and [Origin Cache Control](https://developers.cloudflare.com/cache/concepts/cache-control/).

## Running locally

You need [Rust](https://rustup.rs) (2024 edition) and Postgres 15 or newer.

```bash
createdb illapa
cp .env.example .env    # and adjust DATABASE_URL if needed
cargo run
```

Migrations (tables and INEI population) run by themselves on start. The API listens on `http://127.0.0.1:8080`.

Before sending changes:

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

## Environments

| Environment | Database | Branch |
|---|---|---|
| Local | Postgres on your machine | yours |
| Staging | Supabase project for testing | pull request branch |
| Production | Production Supabase project | `main` |

Configuration goes in environment variables (see [.env.example](.env.example)), never in the code. Staging and production are not deployed yet.

## Data and attribution

This API uses data from SENAMHI (rain, ravine and river flood warnings, what is exposed, and district boundaries), INEI (population, 2017 census) and the [UK Met Office](https://www.metoffice.gov.uk) via [Open-Meteo](https://open-meteo.com) (forecast; the Met Office data is CC BY-SA 4.0: it must be credited and anything derived from it shared under the same licence; Open-Meteo's free API is for non-commercial use only). The AGPL license covers the code, not the data. The license status of each source is in the [site's DATOS.en.md](https://github.com/Illapa-Labs/peru-alerta-lluvia/blob/main/DATOS.en.md): the reuse conditions of SENAMHI and INEI are **still to be confirmed**.

## License

Free software under the [GNU Affero General Public License, version 3 or later](LICENSE) (`AGPL-3.0-or-later`). If you offer this API, or a modified version of it, as a network service, you must offer its complete source code to its users.

Part of the code comes from the Huaico Lima prototype, by the same author, and is published here under this license.

## Contributing

Read [CONTRIBUTING.en.md](CONTRIBUTING.en.md) and the [Code of Conduct](CODE_OF_CONDUCT.en.md).

## Contact

contacto@illapalabs.org
