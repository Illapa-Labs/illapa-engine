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
| `GET /v1/pronostico` | The hourly rain forecast (30 hours) on a grid of about 33 km over Peru, from Open-Meteo. `503` until the first forecast |
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

Field names are in Spanish, like the data they describe. Lists include `actualizado` (when the data was downloaded), `fecha_quebradas` (the date of the ravine warning) and `fuente`. Responses are compressed and can be cached for 5 minutes.

## How it works

Every 30 minutes, in the background:

1. It downloads SENAMHI's rain warning and asks its server, with a spatial query, which districts each area touches.
2. It downloads the ravine activation warning and the populated centers inside those areas, with their population.
3. It matches everything with INEI's districts (2017 census) and saves it in Postgres in a single transaction: if something fails, the previous data keeps being served.

Every 6 hours it asks Open-Meteo for the rain forecast at about 1,450 points over Peru, in batches with pauses to respect its limits (it takes about 3 minutes).

On start it serves the last saved warning and forecast, so there is no need to wait for the first refresh.

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

This API uses data from SENAMHI (warnings and district boundaries), INEI (population, 2017 census) and [Open-Meteo](https://open-meteo.com) (forecast, CC BY 4.0: it must be credited; its free API is for non-commercial use only). The AGPL license covers the code, not the data. The license status of each source is in the [site's DATOS.en.md](https://github.com/Illapa-Labs/peru-alerta-lluvia/blob/main/DATOS.en.md): the reuse conditions of SENAMHI and INEI are **still to be confirmed**.

## License

Free software under the [GNU Affero General Public License, version 3 or later](LICENSE) (`AGPL-3.0-or-later`). If you offer this API, or a modified version of it, as a network service, you must offer its complete source code to its users.

Part of the code comes from the Huaico Lima prototype, by the same author, and is published here under this license.

## Contributing

Read [CONTRIBUTING.en.md](CONTRIBUTING.en.md) and the [Code of Conduct](CODE_OF_CONDUCT.en.md).

## Contact

contacto@illapalabs.org
