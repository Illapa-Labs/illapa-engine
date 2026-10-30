**Español** | [English](DEPLOY.en.md)

# Desplegar la API

La API corre en [Fly.io](https://fly.io) y su base Postgres en [Supabase](https://supabase.com). Delante va Cloudflare como CDN (ver [Caché y CDN](../README.md#caché-y-cdn)). Las contraseñas nunca van en el repositorio: se ponen como secretos de Fly.

## 1. Fly.io

Instala `flyctl` ([guía](https://fly.io/docs/flyctl/install/)) e inicia sesión:

```bash
fly auth login
```

Crea la app (una sola vez). El nombre es único en todo Fly.io: si `illapa-engine` está ocupado, elige otro y cámbialo en `fly.toml`.

```bash
fly apps create illapa-engine --org <tu-organización>
```

## 2. La base en Supabase

1. Crea un proyecto para producción, en la región más cercana a la de Fly (`primary_region` en `fly.toml`).
2. En *Connect*, copia la cadena de conexión del *Session pooler* (puerto 5432). Evita el modo *Transaction* (puerto 6543): no admite las sentencias preparadas que usa la API.
3. Ponla como secreto. Este paso lo hace la persona que tiene la contraseña, en su terminal:

```bash
fly secrets set DATABASE_URL='postgresql://...' --app illapa-engine
```

Las tablas y la población del INEI se crean solas al arrancar (migraciones).

## 3. Desplegar

Desde la carpeta del repositorio:

```bash
fly deploy
fly status
curl https://illapa-engine.fly.dev/healthz
```

`/healthz` responde `ok` enseguida. `/v1/aviso` y las listas responden `503` hasta que termina el primer refresco (un par de minutos), y `/v1/pronostico` hasta el primer pronóstico (unos 3 minutos). Para ver qué hace: `fly logs`.

La máquina queda siempre encendida (`auto_stop_machines = "off"`): si se apagara cuando nadie la visita, dejaría de refrescar los datos. Las visitas las absorbe el CDN.

## 4. El dominio `api.illapalabs.org`

```bash
fly certs add api.illapalabs.org --app illapa-engine
```

`fly certs` dice qué registros DNS crear. En Cloudflare, crea un `CNAME` de `api` hacia `illapa-engine.fly.dev`, **con proxy** (nube naranja), y los registros de validación que pida Fly. Después crea la regla de caché descrita en el README.

## Staging

Para probar antes de producción: otra app de Fly (por ejemplo `illapa-engine-staging`) con su propio proyecto de Supabase, desplegada con `fly deploy --app illapa-engine-staging` y con `ILLAPA_ENV=staging` (`fly secrets set ILLAPA_ENV=staging --app illapa-engine-staging`).
