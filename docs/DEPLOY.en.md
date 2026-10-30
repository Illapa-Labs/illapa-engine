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

## 3. Deploy

From the repository folder:

```bash
fly deploy
fly status
curl https://illapa-engine.fly.dev/healthz
```

`/healthz` answers `ok` right away. `/v1/aviso` and the lists answer `503` until the first refresh finishes (a couple of minutes), and `/v1/pronostico` until the first forecast (about 3 minutes). To see what it is doing: `fly logs`.

The machine stays on (`auto_stop_machines = "off"`): if it stopped when nobody visits, it would stop refreshing the data. The CDN takes the visits.

## 4. The `api.illapalabs.org` domain

```bash
fly certs add api.illapalabs.org --app illapa-engine
```

`fly certs` says which DNS records to create. In Cloudflare, create a `CNAME` from `api` to `illapa-engine.fly.dev`, **proxied** (orange cloud), plus the validation records Fly asks for. Then create the cache rule described in the README.

## Staging

To test before production: another Fly app (for example `illapa-engine-staging`) with its own Supabase project, deployed with `fly deploy --app illapa-engine-staging` and with `ILLAPA_ENV=staging` (`fly secrets set ILLAPA_ENV=staging --app illapa-engine-staging`).
