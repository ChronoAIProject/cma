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
  #threadId: string | undefined;

  constructor(private readonly expectedThreadId?: string) {}

  /** Accept one normalized event and return only a complete unit for delivery. */
  accept(event: AguiEvent): readonly AguiEvent[] {
    this.#acceptThread(event.threadId);
    if (event.ephemeral === true) {
      return [event];
    }
    const firstPrefix = this.#prefixes[0];
    if (firstPrefix !== undefined && event.runId !== firstPrefix.runId) {
      throw new ProtocolError('AG-UI durable projection changed runId before its final frame');
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

  #acceptThread(threadId: string): void {
    const accepted = this.#threadId ?? this.expectedThreadId;
    if (accepted !== undefined && threadId !== accepted) {
      throw new ProtocolError('AG-UI stream carried a frame for another threadId');
    }
    this.#threadId = threadId;
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
