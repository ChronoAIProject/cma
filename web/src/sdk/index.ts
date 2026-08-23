/**
 * The chrono-bot client SDK.
 *
 * One import for everything a consumer needs to drive a chrono-bot deployment:
 * start a session, read what it may do, hold a conversation with it, and be
 * told in words when it refuses.
 *
 * ## Why this exists
 *
 * `CLAUDE.md` §1 states what chrono-bot is for: its clients are other
 * products' frontends and backends, and this repository's own `web/` is the
 * reference client — held to the same constraints a third party is, because a
 * client permitted one private route would prove nothing about the one
 * somebody else has to build. This package is the other half of that. The
 * dashboard consumes exactly what is exported here, so an interface it needs
 * and this does not have is a gap that shows up as a build failure rather than
 * as a discovery six months later.
 *
 * ## Getting started
 *
 * ```ts
 * import { createClient } from '@chronoai/chrono-bot';
 *
 * const bot = createClient({
 *   baseUrl: 'https://bot-api.example',
 *   credential: () => tokenStore.current(),
 *   cookies: false, // cross-origin: do not send the browser's own
 * });
 *
 * const session = await bot.createSession({ cols: 120, rows: 32 });
 * if (!session.capabilities.conversation) {
 *   throw new Error('this deployment did not grant this session a conversation');
 * }
 * ```
 *
 * The dashboard's own configuration is the empty one — same-origin paths and
 * the cookie the browser attaches — which is why every default is that.
 *
 * ## What is not here yet
 *
 * The AG-UI event stream and the conversation fold still live in `web/src/agui`
 * and `web/src/chat`, and are re-exported below rather than re-homed. They are
 * already framework-free and protocol-only, so the move is mechanical; until
 * it happens this directory is not copyable on its own, and saying so is
 * cheaper than letting somebody discover it.
 */

export { Cma, createClient } from './client';
export { resolveConfig, type ClientConfig, type Credential, type ResolvedConfig } from './config';
export { ApiError, CapabilityRefused, DeploymentAtCapacity, SessionLimitReached } from './errors';
export { request, type RequestOptions } from './http';
export {
  readResources,
  toResourceStatus,
  type ResourceStatusReason,
  type SessionResourceStatus,
} from './resources';
export {
  changePlane,
  createSession,
  deleteSession,
  fetchTranscript,
  listKernels,
  listSessions,
  quoteSession,
  readAllowance,
  readSpend,
  readSession,
  reviveSession,
  stopSession,
  terminateSession,
  toSession,
  type CreateSession,
  type PricedQuote,
  type PricedSpend,
  type PricingQuote,
  type SessionAllowance,
  type SessionSpend,
  type UnpricedQuote,
  type UnpricedSpend,
} from './sessions';
export type {
  Kernel,
  KernelOffer,
  Session,
  SessionCreator,
  SessionLifecycle,
  SessionPlane,
  SessionService,
  StopReason,
} from './types';
export {
  KNOWN_EVENTS,
  LIVE_MARKER,
  PATHS,
  PRICING_WIRE,
  STATE_DELTA_PATCHES,
  frameKey,
} from './wire';

/*
 * The conversation half, re-exported from where it still lives. Listed
 * explicitly rather than star-exported so this file states the SDK's surface
 * in one place — a consumer reading it should not have to open two more files
 * to learn what they may call.
 */
export {
  AguiConnection,
  AguiOperations,
  AguiRefusal,
  toCapabilities,
  toEvent,
  type AguiEvent,
  type AguiTransport,
  type ConnectionOptions,
  type Interrupt,
  type LinkStatus,
  type SessionCapabilities,
} from '../agui';
export {
  EMPTY,
  fold,
  reduce,
  type Conversation,
  type Entry,
  type MessageEntry,
  type MemoryPressure,
  type Resources,
  type ToolCallEntry,
  type Usage,
} from '../chat/conversation';
