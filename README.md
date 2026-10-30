**Español** | [English](README.en.md)

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="branding/chakana-mark-for-dark-backgrounds.svg">
    <img alt="Illapa Labs" src="branding/chakana-mark-for-light-backgrounds.svg" width="96">
  </picture>
</p>

# Illapa Engine

[![Licencia: AGPL v3](https://img.shields.io/badge/licencia-AGPL%20v3-blue.svg)](LICENSE)

La API de [Illapa Labs](https://github.com/Illapa-Labs). Pregunta al SENAMHI por el aviso de lluvia y el de activación de quebradas, calcula qué distritos toca cada uno y cuánta gente vive en las zonas con aviso, y lo sirve como JSON listo para usar. La web [Perú Alerta Lluvia](https://github.com/Illapa-Labs/peru-alerta-lluvia) la consumirá en lugar de hacer ese trabajo ella misma.

> **No es un sistema oficial de alerta.** La fuente de los avisos es el [SENAMHI](https://www.senamhi.gob.pe). Ante una emergencia llama al 115 (INDECI) o al 116 (Bomberos).

## Endpoints

| Ruta | Qué devuelve |
|---|---|
| `GET /v1/aviso` | El aviso de lluvia (24 h) en GeoJSON, aligerado para el mapa. `503` hasta el primer refresco |
| `GET /v1/departamentos` | Los 25 departamentos con su población, su nivel más alto y cuántos distritos tienen aviso |
| `GET /v1/provincias` | Las provincias, con los mismos datos |
| `GET /v1/distritos` | Los distritos con su nivel de lluvia y de quebradas, y los centros poblados y personas en zona de aviso, por nivel |
| `GET /healthz` | `ok` si la API responde |

`/v1/provincias` y `/v1/distritos` aceptan estos filtros:

| Filtro | Ejemplo | Qué hace |
|---|---|---|
| `departamento` | `15` o `lima` | Solo ese departamento (ubigeo o nombre) |
| `provincia` | `1501` o `huarochiri` | Solo esa provincia |
| `solo_avisos` | `true` | Solo los que tienen algún aviso |
| `nivel_minimo` | `3` | Solo los de ese nivel o más (2 amarillo, 3 naranja, 4 rojo) |
| `limite` | `100` | Máximo de filas |

Ejemplo: `GET /v1/distritos?departamento=lima&solo_avisos=true`.

Las listas incluyen `actualizado` (cuándo se bajaron los datos), `fecha_quebradas` (la fecha del aviso de quebradas) y `fuente`. Las respuestas van comprimidas y se pueden guardar en caché 5 minutos.

## Cómo funciona

Cada 30 minutos, en segundo plano:

1. Baja el aviso de lluvia del SENAMHI y le pregunta a su servidor, con una consulta espacial, qué distritos toca cada área.
2. Baja el aviso de activación de quebradas y los centros poblados dentro de esas zonas, con su población.
3. Cruza todo con los distritos del INEI (Censo 2017) y lo guarda en Postgres en una sola transacción: si algo falla, se siguen sirviendo los datos anteriores.

Al arrancar sirve el último aviso guardado, así que no hay que esperar al primer refresco.

## Correr en local

Necesitas [Rust](https://rustup.rs) (edición 2024) y Postgres 15 o más reciente.

```bash
createdb illapa
cp .env.example .env    # y ajusta DATABASE_URL si hace falta
cargo run
```

Las migraciones (tablas y población del INEI) se aplican solas al arrancar. La API queda en `http://127.0.0.1:8080`.

Antes de enviar cambios:

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

## Entornos

| Entorno | Base de datos | Rama |
|---|---|---|
| Local | Postgres en tu máquina | la tuya |
| Staging | Proyecto de Supabase para pruebas | rama del *pull request* |
| Producción | Proyecto de Supabase de producción | `main` |

La configuración va en variables de entorno (ver [.env.example](.env.example)); nunca en el código. Staging y producción todavía no están desplegados.

## Datos y atribución

Esta API usa datos del SENAMHI (avisos y límites de distritos) y del INEI (población, Censo 2017). La licencia AGPL cubre el código, no los datos. El estado de la licencia de cada fuente está en [DATOS.md de la web](https://github.com/Illapa-Labs/peru-alerta-lluvia/blob/main/DATOS.md): las condiciones de reutilización de ambas fuentes **todavía están por confirmar**.

## Licencia

Software libre bajo la [GNU Affero General Public License, versión 3 o posterior](LICENSE) (`AGPL-3.0-or-later`). Si ofreces esta API, o una versión modificada, como servicio por red, debes ofrecer su código fuente completo a sus usuarios.

Parte del código viene del prototipo Huaico Lima, del mismo autor, y se publica aquí bajo esta licencia.

## Cómo colaborar

Lee [CONTRIBUTING.md](CONTRIBUTING.md) y el [Código de Conducta](CODE_OF_CONDUCT.md).

## Contacto

contacto@illapalabs.org
