/**
 * The AG-UI client — `docs/API_AGUI.md`.
 *
 * This package is the one place that understands the protocol: its event
 * types, cursor, reconnect order, and typed refusals. Consumers use what is
 * exported here and never implement the wire beside it.
 */

export {
  AguiConnection,
  type AguiTransport,
  type ConnectionOptions,
  type LinkFailure,
  type LinkStatus,
} from './connection.js';
export {
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
} from './events.js';
export {
  AguiOperations,
  NO_CAPABILITIES,
  createOperationTransport,
  fetchTransport,
  toCapabilities,
  type ConcurrentInput,
  type OperationTransport,
  type SessionCapabilities,
} from './operations.js';
export { createAguiTransport, parseEventStream, wireTransport } from './wire.js';

/**
 * Recorded conversations and the driver that replays them.
 *
 * Exported from the client's own index because a driver is how this client is
 * used in development, not a separate thing beside it — the same arrangement
 * `terminal/drivers/` already has.
 */
export { MockSession, type MockOptions } from '../testing/mock.js';
export {
  TRANSCRIPTS,
  transcriptNamed,
  APPROVAL_DENIED,
  APPROVAL_GRANTED,
  FAILED_TOOL_CALL,
  FULL_CONVERSATION,
  INTERRUPTED_TURN,
  NO_USAGE,
  PLAIN_TURN,
  REPORTS_USAGE,
  ROTATES,
  TWO_TOOL_CALLS,
  UNKNOWN_KIND,
} from '../testing/transcripts.js';
export type { Frame, Provenance, Transcript } from '../testing/types.js';
