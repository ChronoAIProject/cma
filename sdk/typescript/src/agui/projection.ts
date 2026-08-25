import { ProtocolError } from '../errors.js';
import type { AguiEvent, UnknownEvent } from './events.js';

/**
 * Assemble wire events into the durable projections acknowledged by one SSE id.
 *
 * A durable prefix has sequence zero until its numbered final frame arrives.
 * Keeping that group intact is what makes occurrence identities possible for
 * unknown siblings that otherwise have no stable semantic key.
 */
export class ProjectionAssembler {
  readonly #prefixes: AguiEvent[] = [];

  /** Accept one normalized event and return only a complete unit for delivery. */
  accept(event: AguiEvent): readonly AguiEvent[] {
    if (event.ephemeral === true) {
      return [event];
    }
    if (event.seq === 0) {
      this.#prefixes.push(event);
      return [];
    }

    const projection = [...this.#prefixes, event];
    this.#prefixes.length = 0;
    return projection.map((candidate, index) => identifyUnknown(candidate, event, index));
  }

  /** A finite source must not stop before a buffered projection gets its cursor. */
  assertComplete(source: string): void {
    if (this.#prefixes.length > 0) {
      throw new ProtocolError(`${source} ended inside a durable projection`);
    }
  }
}

function identifyUnknown(event: AguiEvent, final: AguiEvent, index: number): AguiEvent {
  if (event.type !== 'UNKNOWN') {
    return event;
  }
  const identified: UnknownEvent = {
    ...event,
    projectionId: JSON.stringify([final.threadId, final.seq, index, event.kind]),
  };
  return identified;
}
