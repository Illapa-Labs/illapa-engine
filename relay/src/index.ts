// SPDX-License-Identifier: AGPL-3.0-or-later
// Puente hacia el servidor WFS del SENAMHI. El servidor no acepta conexiones desde donde corre la API (Fly.io, en
// Virginia y en São Paulo), pero sí desde la red de Cloudflare. Este Worker solo reenvía a ese servicio, solo con la
// clave que comparte con la API (no es un proxy abierto) y dice quién consulta en el User-Agent.

const UPSTREAM = 'https://idesep.senamhi.gob.pe/geoserver/wfs'
const USER_AGENT = 'illapa-senamhi-relay (+https://github.com/Illapa-Labs/illapa-engine)'

interface Env {
  /** La clave compartida con la API: `wrangler secret put RELAY_KEY` */
  RELAY_KEY?: string
}

/** Compara las claves sin cortar en la primera diferencia, para no dar pistas por el tiempo de respuesta */
function sameKey(given: string | null, expected: string): boolean {
  if (given === null || given.length !== expected.length) return false
  let difference = 0
  for (let i = 0; i < expected.length; i++) difference |= given.charCodeAt(i) ^ expected.charCodeAt(i)
  return difference === 0
}

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const url = new URL(request.url)
    if (url.pathname !== '/wfs') return new Response('no encontrado', { status: 404 })
    if (!env.RELAY_KEY || !sameKey(request.headers.get('x-illapa-relay-key'), env.RELAY_KEY)) {
      return new Response('no autorizado', { status: 401 })
    }
    if (request.method !== 'GET' && request.method !== 'POST') return new Response('método no permitido', { status: 405 })

    const headers = new Headers({ 'user-agent': USER_AGENT, accept: 'application/json' })
    const contentType = request.headers.get('content-type')
    if (contentType) headers.set('content-type', contentType)
    const upstream = await fetch(`${UPSTREAM}${url.search}`, {
      method: request.method,
      headers,
      // Las consultas por POST son formularios chicos (la consulta espacial de los distritos)
      body: request.method === 'POST' ? await request.text() : undefined,
    })

    return new Response(upstream.body, {
      status: upstream.status,
      headers: {
        'content-type': upstream.headers.get('content-type') ?? 'application/json',
        'cache-control': 'no-store',
      },
    })
  },
}
