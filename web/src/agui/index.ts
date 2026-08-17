/**
 * The AG-UI client — `docs/API_AGUI.md`.
 *
 * One place in `web/` understands this protocol: its event types, its cursor,
 * its reconnect order and its typed refusals. Every chat component consumes
 * what is exported here and never the wire.
 *
 * Beside `api/` rather than inside it, because this is a second protocol and
 * not a second endpoint of the first.
 */

export {
  AguiConnection,
  type AguiTransport,
  type ConnectionOptions,
  type LinkStatus,
} from './connection';
export {
  KNOWN_EVENTS,
  toEvent,
  type AguiEvent,
  type Interrupt,
  type InterruptOption,
  type JsonPatch,
  type KnownEventType,
  type RunError,
  type RunFinished,
  type RunOutcome,
  type RunStarted,
  type StateDelta,
  type TextMessageContent,
  type TextMessageEnd,
  type TextMessageStart,
  type ToolCallEnd,
  type ToolCallStart,
  type UnknownEvent,
} from './events';
export {
  AguiOperations,
  AguiRefusal,
  fetchTransport,
  toCapabilities,
  type ConcurrentInput,
  type OperationTransport,
  type SessionCapabilities,
} from './operations';
export { wireTransport } from './wire';
