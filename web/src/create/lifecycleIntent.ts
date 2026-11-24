import { refreshSandboxCapacity } from '../sandbox/useSandboxCapacity';
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { ProtocolError } from '@cma/client';
import { readGate, retryLifecycleRead } from '../api/readRetry';
import { AgentLifecycleCreateError } from '../api/client';
import { discardSubmittedTaskDraft } from './taskDraft';
import { lifecycleFailure, type LifecycleFailure } from './lifecycleFailure';
import type {
  AgentLifecycleCreateInput,
  AgentLifecycleOperationView,
  CmaWebClient,
} from '../api/client';

export { lifecycleStorageKey, lifecycleScopeForAgent } from './lifecycleIntentStorage';
import {
  loadLifecycleIntent,
  hasPendingLifecycleIntent,
  retainLifecycleIntent,
  type Intent,
  type AgentBinding,
} from './lifecycleIntentStorage';

export function useLifecycleIntent(
  client: CmaWebClient,
  owner?: string,
  workspaceId?: string,
  sandboxId?: string,
  binding?: AgentBinding,
  enabled = true,
) {
  const storageScope = { owner, workspaceId, sandboxId };
  const scope = JSON.stringify([owner ?? null, workspaceId ?? null, sandboxId ?? null, binding]);
  const [loadedScope, setLoadedScope] = useState(scope);
  const [saved, setSaved] = useState(() => loadLifecycleIntent(storageScope, binding));
  const current = useRef(saved.intent);
  const active = useRef<AbortController | undefined>(undefined);
  const submitVersion = useRef(0);
  const observedWait = useRef<AgentLifecycleOperationView | undefined>(undefined);
  const [pending, setPending] = useState(false);
  const [failure, setFailure] = useState<LifecycleFailure>();
  const [operation, setOperation] = useState<AgentLifecycleOperationView>();
  const [connection, setConnection] = useState<AgentLifecycleOperationView['connection']>();
  const [pollVersion, setPollVersion] = useState(0);
  if (loadedScope !== scope) {
    // Another location is another retained request with its own slot. Read
    // that slot and drop every observation of the previous one; nothing from
    // one location may be shown as, or resubmitted for, another.
    setLoadedScope(scope);
    setSaved(loadLifecycleIntent(storageScope, binding));
    setPending(false);
    setFailure(undefined);
    setOperation(undefined);
    setConnection(undefined);
  }
  useLayoutEffect(() => {
    current.current = saved.intent;
  }, [saved.intent]);
  // An unsettled request belongs to the scope that sent it. Leaving that scope
  // abandons only its response; its key stays retained there for recovery.
  const generation = useMemo(() => ({ scope, client, enabled }), [scope, client, enabled]);
  const live = useRef<typeof generation | undefined>(undefined);
  useLayoutEffect(() => {
    live.current = generation.enabled ? generation : undefined;
    return () => {
      if (live.current === generation) live.current = undefined;
      active.current?.abort();
      active.current = undefined;
      observedWait.current = undefined;
    };
  }, [generation]);
  const retain = (expected: Intent | undefined, intent?: Intent, beforeRemove?: () => void) => {
    if (live.current !== generation) return false;
    try {
      retainLifecycleIntent(storageScope, expected, intent, beforeRemove);
      current.current = intent;
      setSaved({ intent });
      return true;
    } catch {
      setSaved((previous) => ({
        ...previous,
        error:
          'The Agent request could not be saved. Copy your task and restore browser session storage before retrying.',
      }));
      return false;
    }
  };
  const receipt = saved.intent?.receipt;
  useEffect(() => {
    if (!enabled || !receipt || !client.readAgentLifecycle) return;
    const abort = new AbortController();
    const requestKey = current.current?.key;
    const currentRequest = () =>
      !abort.signal.aborted &&
      current.current?.key === requestKey &&
      current.current?.receipt?.id === receipt.id;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let phase: string | undefined;
    let phaseStartedAt = Date.now();
    let retryDelay = 500;
    const gate = readGate(client, JSON.stringify(['lifecycle', scope, receipt.id]));
    const poll = async () => {
      const readStartedAt = Date.now();
      const version = submitVersion.current;
      try {
        const value = await gate.run(abort.signal, (signal) =>
          client.readAgentLifecycle!(receipt.id, { signal }),
        );
        if (!currentRequest()) return;
        if (
          value.id !== receipt.id ||
          (current.current?.agentId && value.agent_id !== current.current.agentId)
        )
          throw new ProtocolError('Agent setup identity could not be verified.');
        if (value.phase !== phase) {
          phase = value.phase;
          phaseStartedAt = Date.now();
        }
        setOperation(value);
        observedWait.current =
          version === submitVersion.current &&
          !active.current &&
          value.status === 'waiting_for_connection'
            ? value
            : undefined;
        if (owner && value.status === 'failed' && value.failure?.code === 'sandbox_limit_reached')
          void refreshSandboxCapacity(client, owner);
        setFailure(undefined);
        retryDelay = 500;
        setConnection(value.status === 'waiting_for_connection' ? value.connection : undefined);
        if (value.status === 'queued' || value.status === 'running') {
          // A slow earlier phase must not add a full second to observing Agent readiness.
          // Count the read itself toward the cadence without overlapping requests.
          // Keep each new phase responsive, then back off if that phase stalls.
          const interval = Date.now() - phaseStartedAt < 10000 ? 250 : 1000;
          const elapsed = Math.max(0, Date.now() - readStartedAt);
          timer = setTimeout(() => void poll(), Math.max(0, interval - elapsed));
        }
      } catch (error: unknown) {
        if (!currentRequest()) return;
        observedWait.current = undefined;
        setFailure(
          lifecycleFailure(error, 'status', current.current?.body.initial_message !== undefined),
        );
        if (!retryLifecycleRead(error, abort.signal)) {
          // Lost access or identity invalidates the visible observation, but
          // never erases the retained command or submits a replacement.
          setOperation(undefined);
          setConnection(undefined);
          return;
        }
        timer = setTimeout(() => void poll(), retryDelay);
        retryDelay = Math.min(retryDelay * 2, 5000);
      }
    };
    void poll();
    return () => {
      abort.abort();
      if (timer !== undefined) clearTimeout(timer);
    };
  }, [client, receipt, pollVersion, owner, scope, enabled]);
  const submit = async (body?: AgentLifecycleCreateInput, draftRevision?: string) => {
    if (
      live.current !== generation ||
      active.current ||
      !client.createAgentLifecycle ||
      saved.error
    )
      return;
    // A delayed connection action may only continue the request it was opened for.
    if (body ? current.current : saved.intent?.key !== current.current?.key) return;
    const intent =
      current.current ??
      (body
        ? { key: crypto.randomUUID(), body, ...(draftRevision ? { draftRevision } : {}) }
        : undefined);
    if (!intent || !retain(current.current, intent)) return;
    const abort = new AbortController();
    active.current = abort;
    submitVersion.current += 1;
    observedWait.current = undefined;
    setPending(true);
    setFailure(undefined);
    try {
      const result = await client.createAgentLifecycle(intent.body, {
        idempotencyKey: intent.key,
        signal: abort.signal,
      });
      if (!abort.signal.aborted) {
        if (intent.receipt && result.id !== intent.receipt.id)
          throw new Error('Agent setup identity changed. Your original request has been saved.');
        setOperation(undefined);
        retain(intent, { ...intent, receipt: result });
      }
    } catch (error: unknown) {
      if (abort.signal.aborted) return;
      if (error instanceof AgentLifecycleCreateError && error.connection)
        setConnection(error.connection);
      else {
        setFailure(lifecycleFailure(error, 'submit', intent.body.initial_message !== undefined));
        if (
          error instanceof AgentLifecycleCreateError &&
          ([400, 422].includes(error.status) ||
            (error.status === 429 && error.failure !== undefined)) &&
          !intent.receipt
        )
          retain(intent);
        if (
          owner &&
          error instanceof AgentLifecycleCreateError &&
          error.status === 429 &&
          error.failure
        )
          void refreshSandboxCapacity(client, owner);
      }
    } finally {
      if (active.current === abort) active.current = undefined;
      if (!abort.signal.aborted) setPending(false);
    }
  };
  const waitingIntent =
    owner &&
    !pending &&
    !failure &&
    !saved.error &&
    saved.intent?.receipt?.id === operation?.id &&
    !saved.intent?.agentId &&
    !operation?.agent_id &&
    operation?.status === 'waiting_for_connection' &&
    operation.connection?.target.type === 'sandbox_profile'
      ? saved.intent
      : undefined;
  return {
    intent: saved.intent,
    pending,
    failure:
      saved.error === undefined ? failure : { kind: 'message' as const, message: saved.error },
    operation,
    connection,
    setConnection: (value: AgentLifecycleOperationView['connection']) => {
      if (live.current === generation && saved.intent?.key === current.current?.key)
        setConnection(value);
    },
    submit,
    retryStatus: () => {
      observedWait.current = undefined;
      setPollVersion((value) => value + 1);
    },
    canDiscardWaiting: waitingIntent !== undefined,
    discardWaiting: () => {
      if (
        !owner ||
        !waitingIntent ||
        current.current !== waitingIntent ||
        active.current ||
        observedWait.current !== operation ||
        !retain(waitingIntent, undefined, () =>
          discardSubmittedTaskDraft(
            owner,
            waitingIntent.body,
            waitingIntent.draftRevision,
            'discarded',
          ),
        )
      )
        return false;
      observedWait.current = undefined;
      setOperation(undefined);
      setConnection(undefined);
      return true;
    },
    clear: () => {
      const intent = saved.intent;
      if (!intent || !retain(intent)) return false;
      if (
        owner &&
        intent &&
        !hasPendingLifecycleIntent(storageScope) &&
        operation?.status === 'succeeded' &&
        operation.id === intent.receipt?.id &&
        operation.agent_id &&
        (intent.agentId === undefined || intent.agentId === operation.agent_id)
      )
        discardSubmittedTaskDraft(owner, intent.body, intent.draftRevision);
      return true;
    },
    bindAgent: (agentId: string) => {
      const intent = saved.intent;
      if (
        !intent?.receipt ||
        operation?.id !== intent.receipt.id ||
        operation.agent_id !== agentId ||
        (intent.agentId !== undefined && intent.agentId !== agentId)
      )
        return false;
      return retain(intent, { ...intent, agentId });
    },
    abandon: () => {
      if (live.current !== generation) return;
      live.current = undefined;
      active.current?.abort();
    },
    blocked: saved.error !== undefined,
  };
}
