import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import {
  ProtocolError,
  RequestAborted,
  type ConversationItemsRequest,
  type CreateResponseRequest,
  type OperationOptions,
  type ResponsesConnectionState,
  type ResponsesConversationItem,
  type ResponsesConversationItemList,
  type ResponsesResponse,
  type ResponsesStreamEvent,
  type ResponseToolActivity,
  type SessionCapabilities,
} from '@chronoai/cma-sdk';
import type { Session } from '../sessions/types';
import {
  createResponsesTurn,
  listResponsesConversationItems,
  readResponsesResponse,
} from '../api/client';
import { Composer } from './Composer';
import { MessageStream, type MessageStreamEntry, type ThinkingStreamEntry } from './MessageStream';
import type { ThinkingActivity } from './Thinking';
import { planeStateFor } from './planeState';
import './ConversationSurface.css';
import './ChatSurface.css';

const HISTORY_PAGE_SIZE = 100;
const HISTORY_MAX_PAGES = 100;
const HISTORY_MAX_ITEMS = HISTORY_PAGE_SIZE * HISTORY_MAX_PAGES;

/** The public surface used from the SDK connection and by controlled stream doubles. */
export interface ChatResponsesConnection extends AsyncIterable<ResponsesStreamEvent> {
  readonly state: ResponsesConnectionState;
  cancel(reason?: unknown): void;
}

type ListItems = (
  id: string,
  page?: ConversationItemsRequest,
  signal?: AbortSignal,
) => Promise<ResponsesConversationItemList>;

type CreateTurn = (
  request: CreateResponseRequest,
  options?: OperationOptions,
) => Promise<ChatResponsesConnection>;

interface ChatSurfaceProps {
  readonly session: Session;
  readonly listItems?: ListItems;
  readonly createTurn?: CreateTurn;
  readonly readResponse?: (id: string, signal?: AbortSignal) => Promise<ResponsesResponse>;
  /** Kept in the shared stream if a future Responses extension projects one. */
  readonly approval?: ReactNode;
}

interface LocalTurn {
  readonly token: number;
  readonly userText: string;
  readonly responseId?: string;
  readonly outputText: string;
  readonly reasoningText: string;
  readonly activities: readonly ThinkingActivity[];
  readonly terminal?: ResponsesResponse;
  /** Durable items cover this turn; retain only presentation that items cannot encode. */
  readonly durable?: boolean;
}

interface HistorySnapshot {
  readonly predecessor: string | null;
}

/**
 * Read complete newest-first durable history under explicit progress and size bounds.
 *
 * The API's page is newest first and `after` walks toward older items. Keeping that
 * order here makes one full read indistinguishable from a single large page.
 */
export async function readAllChatItems(
  id: string,
  list: ListItems,
  signal?: AbortSignal,
): Promise<readonly ResponsesConversationItem[]> {
  const items: ResponsesConversationItem[] = [];
  const itemIds = new Set<string>();
  const cursors = new Set<string>();
  let after: string | undefined;

  for (let pageNumber = 0; pageNumber < HISTORY_MAX_PAGES; pageNumber += 1) {
    const page = await list(
      id,
      after === undefined ? { limit: HISTORY_PAGE_SIZE } : { after, limit: HISTORY_PAGE_SIZE },
      signal,
    );
    for (const item of page.data) {
      if (itemIds.has(item.id)) {
        throw new ProtocolError(`conversation ${id} repeated item ${item.id} while paginating`);
      }
      if (items.length >= HISTORY_MAX_ITEMS) {
        throw new ProtocolError(`conversation ${id} exceeded the bounded history size`);
      }
      itemIds.add(item.id);
      items.push(item);
    }
    if (!page.has_more) return items;

    const next = page.last_id;
    if (page.data.length === 0 || next === null || next === after || cursors.has(next)) {
      throw new ProtocolError(`conversation ${id} history pagination made no progress`);
    }
    cursors.add(next);
    after = next;
  }

  throw new ProtocolError(`conversation ${id} exceeded the bounded history page count`);
}

function textOf(item: Extract<ResponsesConversationItem, { readonly type: 'message' }>): string {
  return item.content.map((part) => part.text).join('');
}

function activityOf(item: ResponseToolActivity): ThinkingActivity {
  return {
    id: item.call_id,
    kind: item.kind,
    target: item.target,
    status: item.status,
    ...(item.duration_ms === undefined ? {} : { durationMs: item.duration_ms }),
    ...(item.arguments === undefined ? {} : { arguments: item.arguments }),
    ...(item.result === undefined ? {} : { result: item.result }),
  };
}

/**
 * Turn durable Responses items into the existing stream's ordered presentation model.
 *
 * Tool updates are folded by call id and emitted once, immediately before that turn's
 * assistant message. They therefore never become the Agent surface's top-level rows.
 */
export function projectChatHistory(
  newestFirst: readonly ResponsesConversationItem[],
): readonly MessageStreamEntry[] {
  const projected: MessageStreamEntry[] = [];
  const activities: ThinkingActivity[] = [];
  const activityPositions = new Map<string, number>();
  let turnId: string | undefined;
  let sequence = 0;

  const nextSequence = () => {
    sequence += 1;
    return sequence;
  };
  const flushThinking = () => {
    if (activities.length === 0) return;
    projected.push({
      kind: 'thinking',
      id: `thinking:${turnId ?? activities[0]?.id ?? String(sequence)}`,
      activities: [...activities],
      live: false,
      seq: nextSequence(),
    });
    activities.length = 0;
    activityPositions.clear();
  };

  for (const item of [...newestFirst].reverse()) {
    if (item.type === 'cma_chat.tool_activity') {
      const activity = activityOf(item);
      const existing = activityPositions.get(activity.id);
      if (existing === undefined) {
        activityPositions.set(activity.id, activities.length);
        activities.push(activity);
      } else {
        activities[existing] = activity;
      }
      continue;
    }

    if (item.role === 'user') {
      // A previous failed turn can have tools but no assistant message.
      flushThinking();
      turnId = item.id;
      projected.push({
        kind: 'message',
        id: item.id,
        role: 'user',
        text: textOf(item),
        committed: true,
        seq: nextSequence(),
      });
      continue;
    }

    flushThinking();
    projected.push({
      kind: 'message',
      id: item.id,
      role: 'assistant',
      text: textOf(item),
      committed: true,
      seq: nextSequence(),
    });
    turnId = undefined;
  }
  flushThinking();
  return projected;
}

/**
 * The linear Responses fence represented by the newest durable message.
 *
 * Assistant item ids share the response turn sequence. A terminal turn with no
 * assistant still has its owner item immediately after `TurnStarted`, so its
 * durable sequence is one greater than the response sequence.
 */
export function latestResponseId(
  conversationId: string,
  newestFirst: readonly ResponsesConversationItem[],
): string | null | undefined {
  const prefix = `msg_${conversationId}_`;
  for (const item of newestFirst) {
    if (item.type !== 'message') continue;
    if (!item.id.startsWith(prefix)) return undefined;
    const sequence = Number(item.id.slice(prefix.length));
    if (!Number.isSafeInteger(sequence) || sequence < 1) return undefined;
    const turn = item.role === 'assistant' ? sequence : sequence - 1;
    if (turn < 1) return undefined;
    return `resp_${conversationId}_${String(turn)}`;
  }
  return null;
}

function errorMessage(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}

function responseOutputText(response: ResponsesResponse): string {
  return response.output.flatMap((message) => message.content.map((part) => part.text)).join('');
}

function stateIntoTurn(current: LocalTurn, state: ResponsesConnectionState): LocalTurn {
  return {
    ...current,
    ...(state.response === undefined ? {} : { responseId: state.response.id }),
    outputText: state.outputText,
    reasoningText: state.terminal === undefined ? state.reasoningText : '',
    activities: state.toolActivities.map(activityOf),
    ...(state.terminal === undefined ? {} : { terminal: state.terminal }),
  };
}

function localEntries(turn: LocalTurn | undefined, after: number): readonly MessageStreamEntry[] {
  if (turn === undefined) return [];
  const entries: MessageStreamEntry[] = [];
  let sequence = after;
  if (turn.durable !== true) {
    sequence += 1;
    entries.push({
      kind: 'message',
      id: `local-user:${String(turn.token)}`,
      role: 'user',
      text: turn.userText,
      committed: true,
      seq: sequence,
    });
  }
  if (turn.terminal === undefined) return entries;

  if (turn.durable !== true && turn.activities.length > 0) {
    sequence += 1;
    entries.push({
      kind: 'thinking',
      id: `local-thinking:${String(turn.token)}`,
      activities: turn.activities,
      live: false,
      seq: sequence,
    } satisfies ThinkingStreamEntry);
  }
  if (turn.durable !== true && turn.outputText !== '') {
    sequence += 1;
    entries.push({
      kind: 'message',
      id: turn.terminal.output[0]?.id ?? `local-assistant:${String(turn.token)}`,
      role: 'assistant',
      text: turn.outputText,
      committed: true,
      seq: sequence,
    });
  }
  if (turn.terminal.status === 'failed' || turn.terminal.status === 'incomplete') {
    sequence += 1;
    entries.push({
      kind: 'failure',
      id: `local-failure:${String(turn.token)}`,
      message:
        turn.terminal.error?.message ??
        (turn.terminal.status === 'incomplete'
          ? 'The response ended before it completed.'
          : 'The response failed without a reported reason.'),
      ...(turn.terminal.error === null ? {} : { code: turn.terminal.error.code }),
      seq: sequence,
    });
  }
  return entries;
}

/** The deliberately reduced, Responses-backed conversation surface for Chat. */
export function ChatSurface(props: ChatSurfaceProps) {
  if (props.session.chat === undefined) {
    return (
      <section className="conversation chat-surface" aria-label="Chat conversation">
        <div className="notice notice--fail chat-surface__failure" role="alert">
          This session is not a chat thread.
        </div>
      </section>
    );
  }

  // Keying here also makes direct consumers safe when they switch conversation ids.
  return <ChatSurfaceThread key={props.session.id} {...props} />;
}

function ChatSurfaceThread({
  session,
  listItems = listResponsesConversationItems,
  createTurn = createResponsesTurn,
  readResponse = readResponsesResponse,
  approval,
}: ChatSurfaceProps) {
  const [history, setHistory] = useState<readonly ResponsesConversationItem[]>([]);
  const [historyLoaded, setHistoryLoaded] = useState(false);
  const [predecessor, setPredecessor] = useState<string | null | undefined>(undefined);
  const [historyFailure, setHistoryFailure] = useState<string | undefined>(undefined);
  const [turnFailure, setTurnFailure] = useState<string | undefined>(undefined);
  const [admitting, setAdmitting] = useState(false);
  const [turn, setTurn] = useState<LocalTurn | undefined>(undefined);
  const readGeneration = useRef(0);
  const turnToken = useRef(0);
  const activeConnection = useRef<ChatResponsesConnection | null>(null);
  const activeTurnController = useRef<AbortController | null>(null);

  const refreshHistory = useCallback(
    async (signal?: AbortSignal): Promise<HistorySnapshot | undefined> => {
      const generation = ++readGeneration.current;
      try {
        const items = await readAllChatItems(session.id, listItems, signal);
        const previous = latestResponseId(session.id, items);
        if (previous === undefined) {
          throw new ProtocolError(
            `conversation ${session.id} did not expose a usable latest response identifier`,
          );
        }
        if (signal?.aborted === true || generation !== readGeneration.current) return undefined;
        setHistory(items);
        setPredecessor(previous);
        setHistoryLoaded(true);
        setHistoryFailure(undefined);
        return { predecessor: previous };
      } catch (cause) {
        if (signal?.aborted === true || generation !== readGeneration.current) return undefined;
        setHistoryLoaded(false);
        setHistoryFailure(`This chat's history could not be read: ${errorMessage(cause)}`);
        return undefined;
      }
    },
    [listItems, session.id],
  );

  useEffect(() => {
    const controller = new AbortController();
    queueMicrotask(() => {
      if (!controller.signal.aborted) void refreshHistory(controller.signal);
    });
    return () => {
      controller.abort();
    };
  }, [refreshHistory]);

  useEffect(
    () => () => {
      activeTurnController.current?.abort();
      activeConnection.current?.cancel('chat surface unmounted');
    },
    [],
  );

  const consume = useCallback(
    async (
      connection: ChatResponsesConnection,
      token: number,
      controller: AbortController,
    ): Promise<void> => {
      let streamFailure: unknown;
      try {
        for await (const event of connection) {
          void event;
          setTurn((current) =>
            current?.token === token ? stateIntoTurn(current, connection.state) : current,
          );
        }
      } catch (cause) {
        if (controller.signal.aborted || cause instanceof RequestAborted) return;
        streamFailure = cause;
      }

      try {
        let terminal = connection.state.terminal;
        if (terminal === undefined) {
          const responseId = connection.state.response?.id;
          if (responseId !== undefined) {
            try {
              const recovered = await readResponse(responseId, controller.signal);
              if (recovered.status !== 'in_progress') terminal = recovered;
            } catch (cause) {
              if (controller.signal.aborted || cause instanceof RequestAborted) return;
              if (streamFailure === undefined) throw cause;
              throw new ProtocolError(
                `the Responses stream stopped (${errorMessage(streamFailure)}) and response recovery failed (${errorMessage(cause)})`,
              );
            }
          }
        }
        if (terminal === undefined) {
          if (streamFailure !== undefined) throw streamFailure;
          throw new ProtocolError('the Responses stream ended without a terminal response');
        }

        const durableOutput = responseOutputText(terminal);
        const terminalState: ResponsesConnectionState = {
          ...connection.state,
          outputText: durableOutput === '' ? connection.state.outputText : durableOutput,
          terminal,
        };
        setPredecessor(terminal.id);
        setTurnFailure(undefined);
        setTurn((current) => {
          if (current?.token !== token) return current;
          return stateIntoTurn(current, terminalState);
        });
        const snapshot = await refreshHistory(controller.signal);
        if (snapshot?.predecessor === terminal.id) {
          setTurn((current) => {
            if (current?.token !== token) return current;
            return terminal.status === 'completed' ? undefined : { ...current, durable: true };
          });
        }
      } catch (cause) {
        if (controller.signal.aborted || cause instanceof RequestAborted) return;
        setTurnFailure(`This response stream stopped: ${errorMessage(cause)}`);
        // The server owns an admitted turn after disconnect. Recover whatever
        // has committed rather than removing the request from the transcript.
        await refreshHistory(controller.signal);
      } finally {
        if (activeConnection.current === connection) activeConnection.current = null;
        if (activeTurnController.current === controller) activeTurnController.current = null;
      }
    },
    [readResponse, refreshHistory],
  );

  const send = useCallback(
    async (text: string): Promise<boolean> => {
      if (
        !historyLoaded ||
        predecessor === undefined ||
        (turn !== undefined && turn.terminal === undefined) ||
        admitting
      ) {
        return false;
      }
      setAdmitting(true);
      setTurnFailure(undefined);
      const controller = new AbortController();
      activeTurnController.current = controller;
      const token = ++turnToken.current;
      const request: CreateResponseRequest = {
        conversation: session.id,
        input: text,
        ...(predecessor === null ? {} : { previous_response_id: predecessor }),
      };
      try {
        const connection = await createTurn(request, { signal: controller.signal });
        if (controller.signal.aborted) {
          connection.cancel(controller.signal.reason);
          return false;
        }
        activeConnection.current = connection;
        setTurn({
          token,
          userText: text,
          outputText: '',
          reasoningText: '',
          activities: [],
        });
        void consume(connection, token, controller);
        return true;
      } catch (cause) {
        if (!controller.signal.aborted && !(cause instanceof RequestAborted)) {
          setTurnFailure(`This message was not sent: ${errorMessage(cause)}`);
          await refreshHistory(controller.signal);
        }
        return false;
      } finally {
        setAdmitting(false);
      }
    },
    [admitting, consume, createTurn, historyLoaded, predecessor, refreshHistory, session.id, turn],
  );

  const durableEntries = useMemo(() => projectChatHistory(history), [history]);
  const entries = useMemo(
    () => [...durableEntries, ...localEntries(turn, durableEntries.length)],
    [durableEntries, turn],
  );
  const turnInFlight = turn !== undefined && turn.terminal === undefined;
  const liveThinking =
    turnInFlight && (turn.reasoningText !== '' || turn.activities.length > 0)
      ? { text: turn.reasoningText, activities: turn.activities, live: true }
      : undefined;
  const liveMessage =
    turnInFlight && turn.outputText !== '' ? { text: turn.outputText } : undefined;
  const state = planeStateFor(session, approval !== undefined);
  const failure = turnFailure ?? historyFailure;
  const refusedReason = !historyLoaded
    ? (historyFailure ?? 'Loading this chat history.')
    : predecessor === undefined
      ? "This chat's latest response could not be identified."
      : state.interactive
        ? undefined
        : state.reason;
  const capabilities: SessionCapabilities = {
    ...session.capabilities,
    steer: false,
    interrupt: false,
    concurrentInput: 'reject',
  };

  return (
    <section
      className="conversation chat-surface"
      aria-label="Chat conversation"
      data-surface="chat"
    >
      {failure === undefined ? null : (
        <div className="notice notice--fail chat-surface__failure" role="alert">
          <span className="notice__detail">{failure}</span>
        </div>
      )}

      <MessageStream
        entries={entries}
        turnInFlight={turnInFlight}
        live={liveMessage}
        reasoning={liveThinking}
        linkLive={historyLoaded}
        openingHint={false}
      />

      <div className="conversation__foot chat-surface__foot">
        <div className="conversation__footcolumn">
          {approval ?? (
            <Composer
              capabilities={capabilities}
              commandsEnabled={false}
              turnInFlight={turnInFlight}
              refusedReason={refusedReason}
              busy={admitting}
              onSend={send}
              onInterrupt={() => {}}
            />
          )}
        </div>
      </div>
    </section>
  );
}
