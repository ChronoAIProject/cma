import { responseRequestId, resolveRequestId, type ResolvedConfig } from '../config.js';
import { ApiError, ProtocolError, TransportError } from '../errors.js';
import { raceWithAbortSignal, withOperationDeadline } from '../http.js';
import { identity, query, read, segment } from '../resources.js';
import { eventStream } from '../streaming.js';
import type { V2ResponseStreamOptions } from '../responses-v2.js';
import { V2EventState } from './state.js';
import { V2ObservationError, V2PrematureEof, v2ResponseFrames } from './stream.js';
import { v2ResponseObject, type V2ResponseReceipt, type V2ResponseObject } from './types.js';

const MAX_REPLAY_ATTEMPTS = 4;
const RECOVERY_DEADLINE_MS = 120_000;
const OBSERVATION_UNAVAILABLE = 'response_observation_unavailable';

export interface V2RecoveryNotice {
  readonly receipt: V2ResponseReceipt;
  readonly startingAfter?: number;
  readonly initiatingRequestId: string;
  readonly interruptedRequestId: string;
  readonly replayRequestId: string;
  readonly attempt: number;
  readonly reason: 'observation_error' | 'premature_eof' | 'transport' | 'http';
}

function recoverable(cause: unknown): boolean {
  if (cause instanceof V2ObservationError) return cause.code === OBSERVATION_UNAVAILABLE;
  return cause instanceof TransportError || (cause instanceof ApiError && cause.isRetryable);
}

function interruptionReason(cause: unknown): V2RecoveryNotice['reason'] {
  if (cause instanceof V2ObservationError) return 'observation_error';
  if (cause instanceof V2PrematureEof) return 'premature_eof';
  if (cause instanceof ApiError) return 'http';
  return 'transport';
}

function terminal(response: V2ResponseObject): boolean {
  return ['completed', 'failed', 'incomplete'].includes(response.status);
}

async function delay(milliseconds: number, signal: AbortSignal): Promise<void> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    await raceWithAbortSignal(
      signal,
      () =>
        new Promise<void>((resolve) => {
          timer = setTimeout(resolve, milliseconds);
        }),
    );
  } finally {
    clearTimeout(timer);
  }
}

/** A receipt, its acknowledged cursor and event validation state survive each read-only reconnect. */
export async function submitResponseStream(
  config: ResolvedConfig,
  path: string,
  body: unknown,
  headers: Readonly<Record<string, string>> | undefined,
  options: V2ResponseStreamOptions,
  conversation?: string,
): Promise<V2ResponseObject> {
  let receipt: V2ResponseReceipt | undefined;
  let eventState: V2EventState | undefined;
  let cursor: number | undefined;
  let outcome: V2ResponseObject | undefined;
  let interruptedRequestId = options.requestId;

  const consume = async (
    requestId: string | undefined,
    signal?: AbortSignal,
    admitted?: V2ResponseReceipt,
  ) => {
    interruptedRequestId = requestId;
    const startingAfter = cursor;
    const endpoint = admitted
      ? query(`/api/v2/responses/${segment(admitted.responseId)}/events`, {
          starting_after: startingAfter,
        })
      : path;
    const stream = eventStream(
      config,
      endpoint,
      { ...options, signal, requestId },
      (response, signal) => {
        interruptedRequestId = responseRequestId(response) ?? interruptedRequestId;
        eventState ??= new V2EventState(
          response.headers.get('x-cma-response-id') ?? '',
          conversation,
          false,
        );
        return v2ResponseFrames(response, signal, {
          conversation,
          responseId: admitted?.responseId,
          commandId: admitted?.commandId,
          startingAfter,
          eventState,
          allowEmptyReplay: admitted !== undefined,
          onReceipt: admitted
            ? undefined
            : async (admission) => {
                const captured = { ...admission };
                await options.onReceipt?.(admission);
                receipt = captured;
              },
        });
      },
      'Responses v2',
      admitted ? undefined : body,
      admitted ? undefined : headers,
      (resolved) => {
        interruptedRequestId = resolved;
      },
    );
    for await (const event of stream) {
      // An observation error has no durable ID. The recovery notice identifies
      // that interruption without delivering a second event at the same cursor.
      if (event.type === 'error' && event.code === OBSERVATION_UNAVAILABLE) continue;
      const acknowledgedSequence = event.sequence_number;
      const response = 'response' in event ? event.response : undefined;
      const durable = event.type !== 'error';
      if (response) conversation ??= response.conversation.id;
      try {
        await options.onEvent(event);
      } catch (cause) {
        throw new ProtocolError('Responses consumer did not acknowledge a frame', { cause });
      }
      if (!durable) continue;
      cursor = acknowledgedSequence;
      if (response && terminal(response)) outcome = response;
    }
    return cursor !== startingAfter;
  };

  let failure: unknown;
  try {
    await consume(options.requestId, options.signal);
    if (outcome !== undefined) return outcome;
    throw new V2PrematureEof();
  } catch (cause) {
    if (!receipt || interruptedRequestId === undefined || !recoverable(cause)) throw cause;
    if (outcome !== undefined) return outcome;
    failure = cause;
  }

  const admitted = receipt;
  const initiatingRequestId = interruptedRequestId;
  return withOperationDeadline(
    config,
    { signal: options.signal, timeoutMs: RECOVERY_DEADLINE_MS },
    async (signal) => {
      for (let attempt = 1; attempt <= MAX_REPLAY_ATTEMPTS; attempt++) {
        const replayRequestId = await resolveRequestId(config);
        const notice: V2RecoveryNotice = {
          receipt: { ...admitted },
          startingAfter: cursor,
          initiatingRequestId,
          interruptedRequestId: interruptedRequestId ?? initiatingRequestId,
          replayRequestId,
          attempt,
          reason: interruptionReason(failure),
        };
        try {
          await options.onRecovery?.(notice);
        } catch (cause) {
          throw new ProtocolError('Responses consumer did not acknowledge stream recovery', {
            cause,
          });
        }
        if (attempt > 1 || failure instanceof ApiError) {
          const retryAfter = failure instanceof ApiError ? (failure.retryAfterMs ?? 0) : 0;
          await delay(Math.max(retryAfter, 250 * 2 ** (attempt - 1)), signal);
        }
        try {
          const progressed = await consume(replayRequestId, signal, admitted);
          if (outcome !== undefined) return outcome;
          if (!progressed) {
            const retained = await read(
              config,
              `/api/v2/responses/${segment(admitted.responseId)}`,
              identity(v2ResponseObject, admitted.responseId),
              { ...options, signal, requestId: replayRequestId, cache: 'no-store' },
            );
            if (conversation !== undefined && retained.conversation.id !== conversation)
              throw new ProtocolError('Responses recovery returned a different conversation');
            if (terminal(retained)) return retained;
          }
          throw new V2PrematureEof();
        } catch (cause) {
          if (!recoverable(cause)) throw cause;
          if (cause instanceof ApiError && cause.requestId) interruptedRequestId = cause.requestId;
          if (outcome !== undefined) return outcome;
          failure = cause;
        }
      }
      throw new TransportError('Responses recovery exhausted its replay attempts', {
        cause: failure,
      });
    },
  );
}
