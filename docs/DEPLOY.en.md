[Español](DEPLOY.md) | **English**

# Deploying the API

The API runs on [Fly.io](https://fly.io) and its Postgres database on [Supabase](https://supabase.com). Cloudflare sits in front as a CDN (see [Cache and CDN](../README.en.md#cache-and-cdn)). Passwords never go in the repository: they are set as Fly secrets.

## 1. Fly.io

Install `flyctl` ([guide](https://fly.io/docs/flyctl/install/)) and log in:

```bash
fly auth login
```

Create the app (once). Names are unique across Fly.io: if `illapa-engine` is taken, pick another one and change it in `fly.toml`.

```bash
fly apps create illapa-engine --org <your-organization>
```

## 2. The Supabase database

1. Create a production project in the same region as the Fly app (`primary_region` in `fly.toml`): each refresh writes hundreds of rows, and far from the database it takes much longer. Today: Supabase `sa-east-1` and Fly `gru`, both in São Paulo. The API cannot run in the US: SENAMHI's server does not accept connections from there.
2. Under *Connect*, copy the *Session pooler* connection string (port 5432). Avoid *Transaction* mode (port 6543): it does not support the prepared statements the API uses.
3. Set it as a secret. The person who has the password does this, in their own terminal:

```bash
fly secrets set DATABASE_URL='postgresql://...' --app illapa-engine
```

The tables and INEI population are created on start (migrations).

## 3. The relay to SENAMHI

SENAMHI's WFS server does not accept connections from Fly.io (tested from Virginia and São Paulo: the connection times out), but it does from Cloudflare's network. So the API asks it through a Cloudflare Worker (`relay/`) that only forwards to that service, only with a shared key, and says who is asking in its User-Agent. The API uses it once every half hour.

Deploy the Worker (from `relay/`, logged in to Cloudflare):

```bash
npx wrangler deploy
```

Create the key and set it on both sides, without writing it to any file:

```bash
KEY=$(openssl rand -hex 32) && printf '%s' "$KEY" | npx wrangler secret put RELAY_KEY && fly secrets set SENAMHI_RELAY_KEY="$KEY" --app illapa-engine --stage; unset KEY
```

`SENAMHI_WFS_URL` (in `fly.toml`) points to the Worker. If SENAMHI ever allows direct connections, removing that line and the secret is enough.

## 4. Deploy

From the repository folder:

```bash
fly deploy
fly status
curl https://illapa-engine.fly.dev/healthz
```

`/healthz` answers `ok` right away. `/v1/aviso` and the lists answer `503` until the first refresh finishes (a couple of minutes), and `/v1/pronostico` until the first forecast (about 3 minutes). To see what it is doing: `fly logs`.

The machine stays on (`auto_stop_machines = "off"`): if it stopped when nobody visits, it would stop refreshing the data. The CDN takes the visits.

## 5. The `api.illapalabs.org` domain

```bash
fly certs add api.illapalabs.org --app illapa-engine
```

`fly certs` says which DNS records to create. In Cloudflare, create a `CNAME` from `api` to `illapa-engine.fly.dev`, **proxied** (orange cloud), plus the validation records Fly asks for. Then create the cache rule described in the README.

## 6. Protection against abuse

The API is public and read-only: nothing can be stolen or changed, but someone can try to flood it. These layers make that cheap.

**In the API** (already included, see `src/protection.rs`):
- The data routes only answer requests that carry the secret header `x-illapa-origin`, which Cloudflare adds. Anyone calling `illapa-engine.fly.dev` directly gets `403`. `/healthz` stays open for Fly's health checks.
- Routes without filters refuse any parameter, and `/v1/provincias` and `/v1/distritos` refuse unknown ones (`400`): a random parameter cannot be used to dodge the cache.
- It handles at most 128 requests at a time and none lasts more than 10 seconds; anything beyond that gets `503` at once.

**In Cloudflare** (in the `illapalabs.org` dashboard):

1. **The secret header.** Create the key, store it in Fly and copy it to the clipboard, without writing it to any file:

   ```bash
   KEY=$(openssl rand -hex 32) && fly secrets set ORIGIN_SECRET="$KEY" --app illapa-engine --stage && printf '%s' "$KEY" | pbcopy; unset KEY
   ```

   In *Rules → Transform Rules → Modify Request Header*, create a rule for *Hostname* equal to `api.illapalabs.org` that **sets** (*Set static*) the header `x-illapa-origin` to the copied key. Deploy the rule **before** `fly deploy`: if the API required the header before Cloudflare sent it, it would refuse everything.
2. **The cache key.** In the API's cache rule, under *Cache key → Query string*, choose *Include only* with `departamento`, `provincia`, `nivel_minimo`, `solo_avisos` and `limite`. Any other parameter is ignored and the response comes from the cache.
3. **Per-visitor limit.** In *Security → WAF → Rate limiting rules*, create a rule for `api.illapalabs.org` that counts by IP and blocks for a few seconds anyone above, for example, 60 requests in 10 seconds. The limits each plan allows are in [Cloudflare's documentation](https://developers.cloudflare.com/waf/rate-limiting-rules/).

## Staging

To test before production: another Fly app (for example `illapa-engine-staging`) with its own Supabase project, deployed with `fly deploy --app illapa-engine-staging` and with `ILLAPA_ENV=staging` (`fly secrets set ILLAPA_ENV=staging --app illapa-engine-staging`).
