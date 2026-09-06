import { useCallback, useEffect, useRef, useState } from 'react';
import type { CmaWebClient } from '../api/client';
import { CommandKeys } from '../api/client';
import type {
  AgentDetail,
  AgentEvent,
  AgentLiveEvent,
  AgentUsage,
  SandboxEvent,
  WorkspaceDetail,
} from '../api/types';
import { readSandboxHistory } from '../sandbox/eventHistory';
import {
  conversationFromTranscript,
  foldAgentEvent,
  foldLiveEvent,
  type ConversationState,
} from './conversation';

export interface ReadyAgent {
  readonly detail: AgentDetail;
  readonly workspace: WorkspaceDetail;
  readonly conversation: ConversationState;
  readonly sandboxEvents?: readonly SandboxEvent[];
}

export type AgentResourceState =
  | { readonly status: 'idle' }
  | { readonly status: 'loading' }
  | { readonly status: 'error'; readonly message: string }
  | ({ readonly status: 'ready' } & ReadyAgent);

export type AgentCommand = 'send' | 'interrupt' | 'approval' | 'stop' | 'delete';
export type ApprovalDecision = 'allow' | 'deny' | 'abort';

export interface AgentController {
  readonly state: AgentResourceState;
  readonly pending?: AgentCommand;
  readonly actionError?: string;
  readonly reload: () => void;
  readonly clearActionError: () => void;
  readonly send: (content: string, onAccepted?: () => void) => Promise<void>;
  readonly interrupt: () => Promise<void>;
  readonly approve: (approvalId: string, decision: ApprovalDecision) => Promise<void>;
  readonly stop: () => Promise<void>;
  readonly delete: () => Promise<void>;
}

interface AgentControllerOptions {
  readonly onHierarchyChange?: () => void;
  readonly onDeleted?: () => void;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : 'The Agent operation did not complete.';
}

function withUsage(conversation: ConversationState, usage: AgentUsage): ConversationState {
  return { ...conversation, usage };
}

export async function loadAgent(
  client: CmaWebClient,
  agentId: string,
  signal: AbortSignal,
): Promise<ReadyAgent & { readonly durableEvents: readonly AgentEvent[] }> {
  const [detail, transcript, usage] = await Promise.all([
    client.readAgent(agentId, { signal }),
    client.readAgentTranscript(agentId, { signal }),
    client.readAgentUsage(agentId, { signal }),
  ]);
  if (detail.id !== agentId || transcript.agentId !== agentId || usage.agentId !== agentId) {
    throw new Error('Agent projections referred to different resources.');
  }

  const workspace = await client.readWorkspace(detail.workspace.id, { signal });
  if (workspace.id !== detail.workspace.id || workspace.sandboxId !== detail.sandbox.id) {
    throw new Error('Agent projections referred to different resources.');
  }
  let conversation = conversationFromTranscript({ ...transcript, turns: [], throughSeq: 0 });
  const durableEvents: AgentEvent[] = [];
  let cursor: number | undefined = 0;
  const seen = new Set<number>([0]);
  do {
    const page = await client.readAgentEvents(agentId, { cursor, signal });
    durableEvents.push(...page.items);
    conversation = page.items.reduce(foldAgentEvent, conversation);
    const nextCursor = page.next;
    if (nextCursor !== undefined && (nextCursor <= (cursor ?? 0) || seen.has(nextCursor))) {
      throw new Error('Agent event pagination did not advance.');
    }
    cursor = nextCursor;
    if (cursor !== undefined) seen.add(cursor);
  } while (cursor !== undefined);

  const snapshot = conversationFromTranscript(transcript);
  const recordedRuns = new Set(conversation.messages.map((message) => message.runId));
  const missing = snapshot.messages.filter((message) => !recordedRuns.has(message.runId));
  conversation = {
    ...conversation,
    messages: [
      ...missing.map((message, index) => ({ ...message, order: index - missing.length })),
      ...conversation.messages,
    ],
    cursor: Math.max(conversation.cursor, transcript.throughSeq),
  };
  if (usage.throughSeq >= (conversation.usage?.throughSeq ?? 0))
    conversation = withUsage(conversation, usage);
  return { detail, workspace, conversation, durableEvents };
}

function refreshConversation(
  current: ConversationState,
  next: Awaited<ReturnType<typeof loadAgent>>,
): ConversationState {
  if (current.cursor >= next.conversation.cursor) return current;
  const folded = next.durableEvents.reduce(foldAgentEvent, current);
  const recordedRuns = new Set(folded.messages.map((message) => message.runId));
  const missing = next.conversation.messages.filter((message) => !recordedRuns.has(message.runId));
  return {
    ...folded,
    messages: [...folded.messages, ...missing],
    cursor: next.conversation.cursor,
    usage:
      (next.conversation.usage?.throughSeq ?? 0) >= (folded.usage?.throughSeq ?? 0)
        ? next.conversation.usage
        : folded.usage,
  };
}

function runId(): string {
  return typeof crypto.randomUUID === 'function'
    ? crypto.randomUUID()
    : `run-${String(Date.now())}-${String(Math.random()).slice(2)}`;
}

export function useAgentController(
  client: CmaWebClient,
  agentId: string | undefined,
  options: AgentControllerOptions = {},
): AgentController {
  const [revision, setRevision] = useState(0);
  const [replayRevision, setReplayRevision] = useState(0);
  const [state, setState] = useState<AgentResourceState>(
    agentId === undefined ? { status: 'idle' } : { status: 'loading' },
  );
  const [pending, setPending] = useState<AgentCommand>();
  const [actionError, setActionError] = useState<string>();
  const [replayError, setReplayError] = useState<string>();
  const [keys] = useState(() => new CommandKeys());
  const actionAbort = useRef<AbortController | undefined>(undefined);
  const actionKind = useRef<AgentCommand | undefined>(undefined);
  const replayAbort = useRef<AbortController | undefined>(undefined);
  const liveCursor = useRef(0);
  const hierarchyChanged = useRef(options.onHierarchyChange);
  useEffect(() => {
    hierarchyChanged.current = options.onHierarchyChange;
  }, [options.onHierarchyChange]);

  const applyLive = useCallback(
    (event: AgentLiveEvent) => {
      if (agentId === undefined) return;
      setState((current) => {
        if (current.status !== 'ready' || current.detail.id !== agentId) return current;
        const conversation = foldLiveEvent(current.conversation, event, agentId);
        liveCursor.current = conversation.cursor;
        return { ...current, conversation };
      });
    },
    [agentId],
  );

  useEffect(() => {
    const controller = new AbortController();
    queueMicrotask(() => {
      if (controller.signal.aborted) return;
      setActionError(undefined);
      setReplayError(undefined);
      setPending(undefined);
      if (agentId === undefined) {
        setState({ status: 'idle' });
        return;
      }
      setState({ status: 'loading' });
      void loadAgent(client, agentId, controller.signal)
        .then((ready) => {
          if (controller.signal.aborted) return;
          liveCursor.current = ready.conversation.cursor;
          setState({ status: 'ready', ...ready });
        })
        .catch((error: unknown) => {
          if (!controller.signal.aborted)
            setState({ status: 'error', message: errorMessage(error) });
        });
    });
    return () => {
      controller.abort();
    };
  }, [agentId, applyLive, client, revision]);

  const replayEnabled =
    agentId !== undefined &&
    state.status === 'ready' &&
    state.detail.id === agentId &&
    pending !== 'send' &&
    pending !== 'approval';

  const provisioning =
    state.status === 'ready' &&
    state.detail.id === agentId &&
    (state.detail.status === 'starting' ||
      (state.detail.firstMessage !== undefined &&
        !['completed', 'failed', 'recovery_required'].includes(state.detail.firstMessage.state)));
  useEffect(() => {
    if (!provisioning || agentId === undefined || pending !== undefined) return;
    const request = new AbortController();
    let timer: ReturnType<typeof setTimeout> | undefined;
    let events: readonly SandboxEvent[] = [];
    let observedStatus: string | undefined;
    const poll = async () => {
      try {
        const next = await loadAgent(client, agentId, request.signal);
        events = await readSandboxHistory(client, next.detail.sandbox.id, request.signal, events);
        if (request.signal.aborted) return;
        setState((current) => {
          if (current.status !== 'ready' || current.detail.id !== agentId) return current;
          const conversation = refreshConversation(current.conversation, next);
          liveCursor.current = conversation.cursor;
          return { status: 'ready', ...next, conversation, sandboxEvents: events };
        });
        setActionError((current) =>
          current === 'Preparation progress could not be refreshed. Reconnecting.'
            ? undefined
            : current,
        );
        if (next.detail.status !== 'starting' && observedStatus !== next.detail.status)
          hierarchyChanged.current?.();
        observedStatus = next.detail.status;
        timer = setTimeout(() => void poll(), 1500);
      } catch {
        if (!request.signal.aborted) {
          setActionError('Preparation progress could not be refreshed. Reconnecting.');
          timer = setTimeout(() => void poll(), 5000);
        }
      }
    };
    timer = setTimeout(() => void poll(), 1500);
    return () => {
      request.abort();
      clearTimeout(timer);
    };
  }, [agentId, client, pending, provisioning]);

  useEffect(() => {
    if (!replayEnabled || agentId === undefined) return;
    const controller = new AbortController();
    replayAbort.current = controller;
    queueMicrotask(() => {
      if (!controller.signal.aborted) setReplayError(undefined);
    });
    void client
      .replayAgent(agentId, {
        after: liveCursor.current,
        signal: controller.signal,
        onEvent: (event) => {
          if (!controller.signal.aborted) applyLive(event);
        },
      })
      .catch((error: unknown) => {
        if (controller.signal.aborted) return;
        setReplayError(`Live Agent updates are unavailable. ${errorMessage(error)}`);
      });
    return () => {
      controller.abort();
      if (replayAbort.current === controller) replayAbort.current = undefined;
    };
  }, [agentId, applyLive, client, replayEnabled, replayRevision]);

  useEffect(() => {
    return () => {
      const controller = actionAbort.current;
      controller?.abort();
      if (actionAbort.current === controller) {
        actionAbort.current = undefined;
        actionKind.current = undefined;
      }
    };
  }, [agentId, client]);

  const reload = useCallback(() => {
    setActionError(undefined);
    setReplayError(undefined);
    setState(agentId === undefined ? { status: 'idle' } : { status: 'loading' });
    setRevision((value) => value + 1);
  }, [agentId]);

  const run = useCallback(
    async (
      approval: { readonly id: string; readonly decision: ApprovalDecision } | string,
      onAccepted?: () => void,
    ) => {
      if (agentId === undefined || actionAbort.current !== undefined) return;
      replayAbort.current?.abort();
      const controller = new AbortController();
      actionAbort.current = controller;
      const approving = typeof approval !== 'string';
      actionKind.current = approving ? 'approval' : 'send';
      setPending(approving ? 'approval' : 'send');
      setActionError(undefined);
      let accepted = false;
      try {
        await client.runAgent(
          {
            threadId: agentId,
            runId: runId(),
            ...(approving
              ? {
                  resume: [
                    {
                      interruptId: approval.id,
                      status: 'resolved' as const,
                      payload: { decision: approval.decision },
                    },
                  ],
                }
              : { messages: [{ role: 'user' as const, content: approval }] }),
          },
          {
            signal: controller.signal,
            onEvent: (event) => {
              if (controller.signal.aborted || event.threadId !== agentId) return;
              if (!accepted && event.type !== 'RUN_ERROR') {
                accepted = true;
                onAccepted?.();
              }
              applyLive(event);
            },
          },
        );
      } catch (error: unknown) {
        if (!controller.signal.aborted) setActionError(errorMessage(error));
      } finally {
        if (actionAbort.current === controller) {
          actionAbort.current = undefined;
          actionKind.current = undefined;
          setPending(undefined);
          setReplayRevision((value) => value + 1);
        }
      }
    },
    [agentId, applyLive, client],
  );

  const command = useCallback(
    async (
      name: Exclude<AgentCommand, 'send' | 'approval'>,
      operation: (id: string, key: string, signal: AbortSignal) => Promise<void>,
    ) => {
      const mayPreemptSend = name === 'interrupt' && actionKind.current === 'send';
      if (agentId === undefined || (actionAbort.current !== undefined && !mayPreemptSend)) {
        return;
      }
      if (name === 'interrupt') actionAbort.current?.abort();
      const scope = `agent:${agentId}:${name}`;
      const controller = new AbortController();
      actionAbort.current = controller;
      actionKind.current = name;
      setPending(name);
      setActionError(undefined);
      try {
        await operation(agentId, keys.for(scope), controller.signal);
        keys.settle(scope);
        options.onHierarchyChange?.();
        if (name === 'delete') options.onDeleted?.();
        else reload();
      } catch (error: unknown) {
        if (!controller.signal.aborted) setActionError(errorMessage(error));
      } finally {
        if (actionAbort.current === controller) {
          actionAbort.current = undefined;
          actionKind.current = undefined;
          setPending(undefined);
          if (name === 'interrupt') setReplayRevision((value) => value + 1);
        }
      }
    },
    [agentId, keys, options, reload],
  );

  return {
    state,
    pending,
    actionError: actionError ?? replayError,
    reload,
    clearActionError: () => {
      setActionError(undefined);
      setReplayError(undefined);
    },
    send: (content, onAccepted) => run(content, onAccepted),
    approve: (approvalId, decision) => run({ id: approvalId, decision }),
    interrupt: () =>
      command('interrupt', (id, key, signal) =>
        client.interruptAgent(id, { idempotencyKey: key, signal }),
      ),
    stop: () =>
      command('stop', (id, key, signal) => client.stopAgent(id, { idempotencyKey: key, signal })),
    delete: () =>
      command('delete', (id, key, signal) =>
        client.deleteAgent(id, { idempotencyKey: key, signal }),
      ),
  };
}
