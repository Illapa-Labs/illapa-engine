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

1. Crea un proyecto para producción en la misma región que la app de Fly (`primary_region` en `fly.toml`): cada refresco escribe cientos de filas, y lejos de la base tarda mucho más. Hoy: Supabase `sa-east-1` y Fly `gru`, ambos en São Paulo. La API no puede correr en EE. UU.: el servidor del SENAMHI no acepta conexiones desde allí.
2. En *Connect*, copia la cadena de conexión del *Session pooler* (puerto 5432). Evita el modo *Transaction* (puerto 6543): no admite las sentencias preparadas que usa la API.
3. Ponla como secreto. Este paso lo hace la persona que tiene la contraseña, en su terminal:

```bash
fly secrets set DATABASE_URL='postgresql://...' --app illapa-engine
```

Las tablas y la población del INEI se crean solas al arrancar (migraciones).

## 3. El puente hacia el SENAMHI

El servidor WFS del SENAMHI no acepta conexiones desde Fly.io (probado desde Virginia y São Paulo: la conexión se agota), pero sí desde la red de Cloudflare. Por eso la API le pregunta a través de un Worker de Cloudflare (`relay/`) que solo reenvía a ese servicio, solo con una clave compartida y diciendo quién consulta en el User-Agent. La API lo usa una vez cada media hora.

Desplegar el Worker (desde `relay/`, con sesión de Cloudflare):

```bash
npx wrangler deploy
```

Crear la clave y ponerla en los dos lados, sin que quede escrita en ningún archivo:

```bash
KEY=$(openssl rand -hex 32) && printf '%s' "$KEY" | npx wrangler secret put RELAY_KEY && fly secrets set SENAMHI_RELAY_KEY="$KEY" --app illapa-engine --stage; unset KEY
```

`SENAMHI_WFS_URL` (en `fly.toml`) apunta al Worker. Si el SENAMHI llega a permitir la conexión directa, basta con borrar esa línea y el secreto.

## 4. Desplegar

Desde la carpeta del repositorio:

```bash
fly deploy
fly status
curl https://illapa-engine.fly.dev/healthz
```

`/healthz` responde `ok` enseguida. `/v1/aviso` y las listas responden `503` hasta que termina el primer refresco (un par de minutos), y `/v1/pronostico` hasta el primer pronóstico (unos 3 minutos). Para ver qué hace: `fly logs`.

La máquina queda siempre encendida (`auto_stop_machines = "off"`): si se apagara cuando nadie la visita, dejaría de refrescar los datos. Las visitas las absorbe el CDN.

## 5. El dominio `api.illapalabs.org`

```bash
fly certs add api.illapalabs.org --app illapa-engine
```

`fly certs` dice qué registros DNS crear. En Cloudflare, crea un `CNAME` de `api` hacia `illapa-engine.fly.dev`, **con proxy** (nube naranja), y los registros de validación que pida Fly. Después crea la regla de caché descrita en el README.

## 6. Protección contra abusos

La API es pública y de solo lectura: no se puede robar ni cambiar nada, pero sí intentar saturarla. Estas capas lo hacen barato.

**En la API** (ya incluido, ver `src/protection.rs`):
- Las rutas de datos solo responden a peticiones que traen la cabecera secreta `x-illapa-origin`, que agrega Cloudflare. Quien llame directo a `illapa-engine.fly.dev` recibe `403`. `/healthz` sigue abierta para los chequeos de Fly.
- Las rutas sin filtros rechazan cualquier parámetro, y `/v1/provincias` y `/v1/distritos` los que no conocen (`400`): así un parámetro al azar no sirve para esquivar la caché.
- Atiende como mucho 128 peticiones a la vez y ninguna dura más de 10 segundos; lo que pase de ahí recibe `503` al instante.

**En Cloudflare** (en el panel de `illapalabs.org`):

1. **La cabecera secreta.** Crea la clave, guárdala en Fly y cópiala al portapapeles, sin escribirla en ningún archivo:

   ```bash
   KEY=$(openssl rand -hex 32) && fly secrets set ORIGIN_SECRET="$KEY" --app illapa-engine --stage && printf '%s' "$KEY" | pbcopy; unset KEY
   ```

   En *Rules → Transform Rules → Modify Request Header*, crea una regla para *Hostname* igual a `api.illapalabs.org` que **ponga** (*Set static*) la cabecera `x-illapa-origin` con la clave copiada. Despliega la regla **antes** de `fly deploy`: si la API empieza a exigir la cabecera antes de que Cloudflare la mande, rechazaría todo.
2. **La clave de caché.** En la regla de caché de la API, en *Cache key → Query string*, elige *Include only* con `departamento`, `provincia`, `nivel_minimo`, `solo_avisos` y `limite`. Cualquier otro parámetro se ignora y la respuesta sale de la caché.
3. **Límite por visitante.** En *Security → WAF → Rate limiting rules*, crea una regla para `api.illapalabs.org` que cuente por IP y bloquee unos segundos a quien pase de, por ejemplo, 60 peticiones en 10 segundos. Los límites que admite cada plan están en la [documentación de Cloudflare](https://developers.cloudflare.com/waf/rate-limiting-rules/).

## Staging

Para probar antes de producción: otra app de Fly (por ejemplo `illapa-engine-staging`) con su propio proyecto de Supabase, desplegada con `fly deploy --app illapa-engine-staging` y con `ILLAPA_ENV=staging` (`fly secrets set ILLAPA_ENV=staging --app illapa-engine-staging`).
