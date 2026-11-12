import { PROTOCOL, serverEventSchema } from '@lila/shared'
import { useQueryClient } from '@tanstack/react-query'
import { useEffect, useState } from 'react'

export type Connection = 'connecting' | 'online' | 'offline' | 'mismatch'

const RETRY_MS = 1500

/**
 * Keeps one WebSocket to the server. Events only say what changed, so each one
 * just marks the matching queries stale; reconnecting refetches everything.
 */
export function useServerEvents(): Connection {
  const queryClient = useQueryClient()
  const [connection, setConnection] = useState<Connection>('connecting')

  useEffect(() => {
    let socket: WebSocket | undefined
    let retry: ReturnType<typeof setTimeout> | undefined
    let closed = false

    const connect = () => {
      const scheme = location.protocol === 'https:' ? 'wss' : 'ws'
      socket = new WebSocket(`${scheme}://${location.host}/api/events`)
      socket.onmessage = (message) => {
        const parsed = serverEventSchema.safeParse(JSON.parse(String(message.data)))
        if (!parsed.success) return
        const event = parsed.data
        if (event.event === 'hello') {
          setConnection(event.protocol === PROTOCOL ? 'online' : 'mismatch')
          void queryClient.invalidateQueries()
        } else {
          void queryClient.invalidateQueries({ queryKey: [event.topic] })
        }
      }
      socket.onclose = () => {
        if (closed) return
        setConnection('offline')
        retry = setTimeout(connect, RETRY_MS)
      }
    }
    connect()

    return () => {
      closed = true
      clearTimeout(retry)
      socket?.close()
    }
  }, [queryClient])

  return connection
}
