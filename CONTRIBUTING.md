**Español** | [English](CONTRIBUTING.en.md)

# Cómo colaborar

Gracias por querer ayudar. Illapa Engine es la API de un proyecto comunitario de código abierto: no tiene anuncios, no se monetiza y busca ser útil para quien vive o viaja por zonas de lluvias fuertes en el Perú.

Al participar aceptas el [Código de Conducta](CODE_OF_CONDUCT.md).

## Qué ayuda más

- Avisar de distritos que salen sin aviso cuando deberían tenerlo, o al revés, con el ubigeo y la fecha.
- Revisar los cruces de nombres entre el SENAMHI y el INEI.
- Pruebas, rendimiento y robustez ante fallas del servidor del SENAMHI.
- Revisar y documentar las licencias de las fuentes de datos.

## Ramas y flujo de trabajo

Usamos [GitHub Flow](https://docs.github.com/es/get-started/using-github/github-flow): una sola rama principal y ramas cortas por cambio.

- `main` es la rama de producción: siempre debe compilar y poder desplegarse. Está protegida y no se escribe en ella directamente.
- Cada cambio va en una rama creada desde `main`, con un nombre del tipo `tipo/descripcion-corta`: `feat/forecast-endpoint`, `fix/district-names`.
- Si no formas parte del equipo, trabaja desde tu *fork* y envía el *pull request* a `main`.
- Para fusionar, un *pull request* necesita al menos una aprobación y que pasen las revisiones. Se fusiona con *squash merge*, así que el título debe seguir Conventional Commits en inglés: será el mensaje del commit en `main`.

## Antes de enviar

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Si cambias el esquema de la base, agrega una migración nueva en `migrations/`; no edites las que ya existen.

## Estilo

- Los mensajes de commit siguen [Conventional Commits](https://www.conventionalcommits.org) en inglés, por ejemplo `feat(api): add the forecast endpoint`.
- Los nombres en el código y los comentarios técnicos van en inglés. Los campos de la API y de la base van en español, como los datos que describen (`ubigeo`, `aviso`, `distrito`).
- Las unidades van en el nombre del campo cuando hay unidades (`pp_mm`, `area_km2`).
- No inventes datos: si falta una fuente, la respuesta lo dice; nunca se rellena con cero.
- Nada de secretos en el repositorio: la configuración va en variables de entorno.

## Licencia de tus aportes

El proyecto se distribuye bajo la [AGPL-3.0-or-later](LICENSE). Al enviar un aporte aceptas que se publique bajo esa misma licencia; sigues siendo la persona autora de lo que escribes. No incluyas código ni datos que no puedas publicar bajo la AGPL.

## Seguridad

Si encuentras un problema de seguridad, no lo publiques en un *issue*: escribe a contacto@illapalabs.org.
