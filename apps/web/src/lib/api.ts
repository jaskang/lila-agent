import type { AppType } from '@lila/server'
import { type ClientResponse, hc } from 'hono/client'
import type { SuccessStatusCode } from 'hono/utils/http-status'

/** Typed against the server's routes: a route change there is a type error here. */
export const api = hc<AppType>('/').api

/** The body of a route's 2xx responses, leaving out its error shapes. */
type Ok<R> = R extends ClientResponse<infer Body, infer Status, 'json'>
  ? Status extends SuccessStatusCode
    ? Body
    : never
  : never

/** Resolves to the 2xx body; throws the server's `{ error }` text for anything else. */
export async function unwrap<R extends ClientResponse<unknown, number, string>>(
  response: Promise<R>,
): Promise<Ok<R>> {
  const res = await response
  if (res.ok) return (await res.json()) as Ok<R>
  const body: unknown = await res.json().catch(() => null)
  const message =
    body && typeof body === 'object' && 'error' in body && typeof body.error === 'string'
      ? body.error
      : `${res.status} ${res.statusText}`
  throw new Error(message)
}
