import { z } from 'zod'

/** Bumped whenever the HTTP or WebSocket contract changes incompatibly. */
export const PROTOCOL = 1

export const DEFAULT_PORT = 4780

export const healthSchema = z.object({
  ok: z.literal(true),
  version: z.string(),
  protocol: z.number().int(),
  pid: z.number().int(),
})
export type Health = z.infer<typeof healthSchema>

export const projectSchema = z.object({
  id: z.string(),
  name: z.string(),
  path: z.string(),
  createdAt: z.number().int(),
})
export type Project = z.infer<typeof projectSchema>

export const newProjectSchema = z.object({
  path: z.string().trim().min(1),
  name: z.string().trim().min(1).optional(),
})
export type NewProject = z.infer<typeof newProjectSchema>

/**
 * Pushed over the WebSocket. Events only say that something changed; clients
 * fetch the data itself over HTTP, so a missed event never leaves them wrong
 * for longer than the next one.
 */
export const serverEventSchema = z.discriminatedUnion('event', [
  z.object({ event: z.literal('hello'), protocol: z.number().int(), seq: z.number().int() }),
  z.object({ event: z.literal('changed'), topic: z.enum(['projects']), seq: z.number().int() }),
])
export type ServerEvent = z.infer<typeof serverEventSchema>
