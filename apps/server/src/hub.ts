import type { ServerEvent } from '@lila/shared'

type Listener = (event: ServerEvent) => void
type Topic = Extract<ServerEvent, { event: 'changed' }>['topic']

/** Fans "something changed" out to every connected client. */
export class Hub {
  #seq = 0
  #listeners = new Set<Listener>()

  get seq(): number {
    return this.#seq
  }

  subscribe(listener: Listener): () => void {
    this.#listeners.add(listener)
    return () => this.#listeners.delete(listener)
  }

  changed(topic: Topic): void {
    const event: ServerEvent = { event: 'changed', topic, seq: ++this.#seq }
    for (const listener of this.#listeners) listener(event)
  }
}
