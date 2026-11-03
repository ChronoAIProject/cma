import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { ApiError, ProtocolError } from '@chronoai/cma-sdk';
import { AgentLifecycleCreateError } from '../api/client';
import { requireTaskStorage } from './taskIdentity';
import type {
  AgentLifecycleCreateInput,
  AgentLifecycleCreateReceipt,
  AgentLifecycleOperationView,
  CmaWebClient,
} from '../api/client';

interface Intent {
  readonly key: string;
  readonly body: AgentLifecycleCreateInput;
  readonly receipt?: AgentLifecycleCreateReceipt;
  readonly agentId?: string;
}
export function lifecycleStorageKey(owner: string, workspaceId?: string, sandboxId?: string) {
  return `cma:agent-creation:${JSON.stringify(sandboxId !== undefined ? [location.origin, owner, 'sandbox', sandboxId] : workspaceId === undefined ? [location.origin, owner] : [location.origin, owner, workspaceId])}`;
}
function load(
  owner?: string,
  workspaceId?: string,
  sandboxId?: string,
): { intent?: Intent; error?: string } {
  try {
    requireTaskStorage();
    if (!owner) throw new Error();
    const raw = sessionStorage.getItem(lifecycleStorageKey(owner, workspaceId, sandboxId));
    if (raw === null) return {};
    const saved = JSON.parse(raw) as { version: number; intent: Intent };
    const intent = saved.intent;
    if (
      saved.version !== 1 ||
      !intent ||
      !/^[a-zA-Z0-9-]{1,200}$/.test(intent.key) ||
      typeof intent.body?.name !== 'string' ||
      !intent.body.name ||
      (intent.body.initial_message !== undefined &&
        typeof intent.body.initial_message !== 'string') ||
      (intent.body.repository !== undefined &&
        (!intent.body.repository ||
          typeof intent.body.repository.url !== 'string' ||
          (intent.body.repository.ref !== undefined &&
            typeof intent.body.repository.ref !== 'string'))) ||
      intent.body.workspace_id !== workspaceId ||
      intent.body.sandbox_id !== sandboxId ||
      (intent.agentId !== undefined && (typeof intent.agentId !== 'string' || !intent.agentId)) ||
      (intent.receipt !== undefined &&
        (typeof intent.receipt.id !== 'string' || !intent.receipt.id))
    )
      throw new Error();
    return { intent };
  } catch {
    return {
      error:
        'The saved Agent request could not be read. Restore your browser session before continuing.',
    };
  }
}

/** Find only a validated creation already bound to this viewer's admitted Agent. */
export function lifecycleScopeForAgent(owner: string, agentId: string) {
  try {
    requireTaskStorage();
    for (let index = 0; index < sessionStorage.length; index += 1) {
      const key = sessionStorage.key(index);
      if (!key?.startsWith('cma:agent-creation:')) continue;
      const raw = sessionStorage.getItem(key);
      if (!raw) continue;
      let body: AgentLifecycleCreateInput | undefined;
      try {
        body = (JSON.parse(raw) as { intent?: Intent }).intent?.body;
      } catch {
        continue;
      }
      if (!body || key !== lifecycleStorageKey(owner, body.workspace_id, body.sandbox_id)) continue;
      const { intent } = load(owner, body.workspace_id, body.sandbox_id);
      if (intent?.agentId === agentId && intent.receipt)
        return { workspaceId: body.workspace_id, sandboxId: body.sandbox_id };
    }
  } catch {
    /* Unreadable storage never authorizes recovery. */
  }
  return undefined;
}

export function useLifecycleIntent(
  client: CmaWebClient,
  owner?: string,
  workspaceId?: string,
  sandboxId?: string,
) {
  const scope = JSON.stringify([owner ?? null, workspaceId ?? null, sandboxId ?? null]);
  const [loadedScope, setLoadedScope] = useState(scope);
  const [saved, setSaved] = useState(() => load(owner, workspaceId, sandboxId));
  const current = useRef(saved.intent);
  const active = useRef<AbortController | undefined>(undefined);
  const [pending, setPending] = useState(false);
  const [failure, setFailure] = useState<string>();
  const [operation, setOperation] = useState<AgentLifecycleOperationView>();
  const [connection, setConnection] = useState<AgentLifecycleOperationView['connection']>();
  const [pollVersion, setPollVersion] = useState(0);
  if (loadedScope !== scope) {
    // Another location is another retained request with its own slot. Read
    // that slot and drop every observation of the previous one; nothing from
    // one location may be shown as, or resubmitted for, another.
    setLoadedScope(scope);
    setSaved(load(owner, workspaceId, sandboxId));
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
  useEffect(() => () => active.current?.abort(), [scope]);
  const retain = (intent?: Intent) => {
    try {
      requireTaskStorage();
      if (!owner) throw new Error();
      if (intent)
        sessionStorage.setItem(
          lifecycleStorageKey(owner, workspaceId, sandboxId),
          JSON.stringify({ version: 1, intent }),
        );
      else sessionStorage.removeItem(lifecycleStorageKey(owner, workspaceId, sandboxId));
      current.current = intent;
      setSaved({ intent });
      return true;
    } catch {
      setSaved((previous) => ({
        ...previous,
        error:
          'The Agent request could not be saved. Restore browser session storage before retrying.',
      }));
      return false;
    }
  };
  const receipt = saved.intent?.receipt;
  useEffect(() => {
    if (!receipt || !client.readAgentLifecycle) return;
    const abort = new AbortController();
    let timer: ReturnType<typeof setTimeout> | undefined;
    let phase: string | undefined;
    let phaseStartedAt = Date.now();
    let retryDelay = 500;
    const poll = async () => {
      const readStartedAt = Date.now();
      try {
        const value = await client.readAgentLifecycle!(receipt.id, { signal: abort.signal });
        if (abort.signal.aborted) return;
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
        if (abort.signal.aborted) return;
        setFailure(error instanceof Error ? error.message : 'Status unavailable.');
        if (
          error instanceof ProtocolError ||
          (error instanceof ApiError && !error.isRetryable && error.status !== 408)
        ) {
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
  }, [client, receipt, pollVersion]);
  const submit = async (body?: AgentLifecycleCreateInput) => {
    if (active.current || !client.createAgentLifecycle || saved.error) return;
    const intent = current.current ?? (body ? { key: crypto.randomUUID(), body } : undefined);
    if (!intent || !retain(intent)) return;
    const abort = new AbortController();
    active.current = abort;
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
        retain({ ...intent, receipt: result });
      }
    } catch (error: unknown) {
      if (abort.signal.aborted) return;
      if (error instanceof AgentLifecycleCreateError && error.connection)
        setConnection(error.connection);
      else {
        setFailure(error instanceof Error ? error.message : 'Agent creation failed.');
        if (
          error instanceof AgentLifecycleCreateError &&
          [400, 422].includes(error.status) &&
          !intent.receipt
        )
          retain();
      }
    } finally {
      active.current = undefined;
      if (!abort.signal.aborted) setPending(false);
    }
  };
  return {
    intent: saved.intent,
    pending,
    failure: saved.error ?? failure,
    operation,
    connection,
    setConnection,
    submit,
    retryStatus: () => setPollVersion((value) => value + 1),
    clear: () => retain(),
    bindAgent: (agentId: string) => {
      const intent = current.current;
      if (
        !intent?.receipt ||
        operation?.id !== intent.receipt.id ||
        operation.agent_id !== agentId ||
        (intent.agentId !== undefined && intent.agentId !== agentId)
      )
        return false;
      return retain({ ...intent, agentId });
    },
    blocked: saved.error !== undefined,
  };
}
