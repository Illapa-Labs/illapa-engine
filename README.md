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
| `GET /v1/pronostico` | La lluvia pronosticada hora por hora (36 horas) en una malla de 0,2° (unos 22 km) sobre el Perú y, en el campo `zonas`, en mallas de 0,1° (la resolución real del modelo, unos 11 km) sobre las zonas con quebradas: Lima Este, costa norte, Áncash, Cusco y Arequipa. Es el modelo global de 10 km del UK Met Office vía Open-Meteo (CC BY-SA 4.0). `503` hasta el primer pronóstico |
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

Las listas incluyen `actualizado` (cuándo se bajaron los datos), `fecha_quebradas` (la fecha del aviso de quebradas) y `fuente`. Las respuestas van comprimidas; cuánto se pueden guardar está en [Caché y CDN](#caché-y-cdn). Hasta el primer refresco, las listas responden `503`: no saber todavía no es lo mismo que no tener avisos.

## Cómo funciona

Cada 30 minutos, en segundo plano:

1. Baja el aviso de lluvia del SENAMHI y le pregunta a su servidor, con una consulta espacial, qué distritos toca cada área.
2. Baja el aviso de activación de quebradas y los centros poblados dentro de esas zonas, con su población.
3. Cruza todo con los distritos del INEI (Censo 2017) y lo guarda en Postgres en una sola transacción: si algo falla, se siguen sirviendo los datos anteriores.

Cada 12 horas pide a Open-Meteo el pronóstico de lluvia del UK Met Office para unos 3 070 puntos sobre el Perú (malla de 0,2°) y unos 800 más en las zonas de 0,1° (`data/zonas-pronostico.json`, que se regenera con `scripts/generar-zonas-pronostico.py`), en tandas con pausas para respetar sus límites (tarda unos 10 minutos). Se cuenta cada punto como una consulta, porque no está claro si Open-Meteo lo hace así: dos corridas al día son unas 7 700 de las 10 000 diarias del plan gratuito, y una prueba comprueba que no se pase. Para agregar una zona hay que sacar puntos de otra o pedir menos corridas.

Al arrancar sirve el último aviso y el último pronóstico guardados, así que no hay que esperar al primer refresco.

## Caché y CDN

Los datos no cambian con cada visita: el aviso cada media hora y el pronóstico cada 12 horas. Por eso la API arma las listas en memoria después de cada refresco (ninguna petición consulta la base ni al SENAMHI) y le dice al navegador y al CDN cuánto pueden guardar cada respuesta:

| Datos | Navegador (`Cache-Control`) | CDN (`CDN-Cache-Control`) |
|---|---|---|
| Aviso, departamentos, provincias, distritos | 5 min, y 25 min más sirviendo la copia anterior mientras pide la nueva | 10 min, y 20 min más sirviendo la anterior |
| Pronóstico | 30 min, y 1 h más sirviendo la anterior | igual |
| Errores (`503`) y `/healthz` | no se guardan (`no-store`) | no se guardan |

Todas las respuestas con datos permiten además seguir sirviendo la última copia hasta un día si la API falla (`stale-if-error`): un aviso de hace un rato es mejor que una pantalla vacía. No se usa `s-maxage`, porque impide que el CDN sirva la copia anterior mientras pide la nueva.

### Regla de Cloudflare (al desplegar)

Con la API detrás de Cloudflare, miles de visitas se convierten en unas pocas peticiones a la API cada 10 minutos. Pero **Cloudflare no guarda respuestas JSON por defecto**: hace falta una regla.

1. El registro DNS de `api.illapalabs.org` debe ir **con proxy** (nube naranja).
2. En *Caching → Cache Rules*, crear una regla para `api.illapalabs.org` cuya ruta empiece con `/v1/`, con **Eligible for cache** y respetando las cabeceras del origen (*Edge TTL* y *Browser TTL*: usar el `Cache-Control` del origen).
3. No activar *Always Online* para esta ruta: con él, Cloudflare ignora `stale-while-revalidate` y `stale-if-error`.
4. Comprobar: `curl -sI https://api.illapalabs.org/v1/aviso | grep -i cf-cache-status` debe dar `HIT` a partir de la segunda petición (`MISS` la primera, `UPDATING` mientras renueva).

Los nombres de las opciones en el panel de Cloudflare cambian con el tiempo: si no coinciden, ver [Cache Rules](https://developers.cloudflare.com/cache/how-to/cache-rules/settings/) y [Origin Cache Control](https://developers.cloudflare.com/cache/concepts/cache-control/).

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

Esta API usa datos del SENAMHI (avisos y límites de distritos), del INEI (población, Censo 2017) y del [UK Met Office](https://www.metoffice.gov.uk) vía [Open-Meteo](https://open-meteo.com) (pronóstico; los datos del Met Office son CC BY-SA 4.0: hay que citarlos y compartir lo derivado con la misma licencia; la API gratuita de Open-Meteo es solo para uso no comercial). La licencia AGPL cubre el código, no los datos. El estado de la licencia de cada fuente está en [DATOS.md de la web](https://github.com/Illapa-Labs/peru-alerta-lluvia/blob/main/DATOS.md): las condiciones de reutilización del SENAMHI y del INEI **todavía están por confirmar**.

## Licencia

Software libre bajo la [GNU Affero General Public License, versión 3 o posterior](LICENSE) (`AGPL-3.0-or-later`). Si ofreces esta API, o una versión modificada, como servicio por red, debes ofrecer su código fuente completo a sus usuarios.

Parte del código viene del prototipo Huaico Lima, del mismo autor, y se publica aquí bajo esta licencia.

## Cómo colaborar

Lee [CONTRIBUTING.md](CONTRIBUTING.md) y el [Código de Conducta](CODE_OF_CONDUCT.md).

## Contacto

contacto@illapalabs.org
