import { KNOWN_EVENTS } from '../wire.js';

const knownEvents = new Set<string>(KNOWN_EVENTS);

/**
 * Whether an unnumbered frame is an interface boundary rather than liveness or
 * part of a durable projection.
 *
 * A continuation has no new durable turn to project, but its response still
 * opens with `RUN_STARTED`. The frame is deliberately cursorless and non-live:
 * it is delivered as an ordering boundary without becoming reconnect state.
 */
export function isCursorlessInterfaceBoundary(
  kind: string,
  seq: number,
  ephemeral: boolean | undefined,
): boolean {
  return kind === 'RUN_STARTED' && seq === 0 && !ephemeral;
}

/**
 * Whether a durable frame may precede the frame that carries its projection's
 * cursor.
 *
 * Unknown kinds are accepted deliberately. The server's event vocabulary grows
 * additively, so an older SDK cannot know whether a future kind is a projection
 * prefix. Treating it as one preserves the frame and leaves the cursor at the
 * last completed projection. Known singleton and final kinds remain strict so
 * a missing id on today's protocol is still reported as malformed.
 */
export function canBeDurableProjectionPrefix(kind: string): boolean {
  if (!knownEvents.has(kind)) {
    return true;
  }
  switch (kind) {
    case 'RUN_FINISHED':
    case 'TEXT_MESSAGE_START':
    case 'TEXT_MESSAGE_CONTENT':
    case 'TOOL_CALL_START':
    case 'TOOL_CALL_END':
      return true;
    default:
      return false;
  }
}
