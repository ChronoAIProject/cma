import { useEffect, useRef, useState } from 'react';
import { AgentLifecycleCreateError } from '../api/client';
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
}
export function lifecycleStorageKey(owner: string, workspaceId?: string) {
  return `cma:agent-creation:${JSON.stringify(workspaceId === undefined ? [location.origin, owner] : [location.origin, owner, workspaceId])}`;
}
function load(owner?: string, workspaceId?: string): { intent?: Intent; error?: string } {
  try {
    if (!owner) throw new Error();
    const raw = sessionStorage.getItem(lifecycleStorageKey(owner, workspaceId));
    if (raw === null) return {};
    const saved = JSON.parse(raw) as { version: number; intent: Intent };
    const intent = saved.intent;
    if (
      saved.version !== 1 ||
      !intent ||
      !/^[a-zA-Z0-9-]{1,200}$/.test(intent.key) ||
      typeof intent.body?.name !== 'string' ||
      !intent.body.name ||
      intent.body.workspace_id !== workspaceId ||
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

export function useLifecycleIntent(client: CmaWebClient, owner?: string, workspaceId?: string) {
  const [saved, setSaved] = useState(() => load(owner, workspaceId));
  const current = useRef(saved.intent);
  const active = useRef<AbortController | undefined>(undefined);
  const [pending, setPending] = useState(false);
  const [failure, setFailure] = useState<string>();
  const [operation, setOperation] = useState<AgentLifecycleOperationView>();
  const [connection, setConnection] = useState<AgentLifecycleOperationView['connection']>();
  const [pollVersion, setPollVersion] = useState(0);
  useEffect(() => () => active.current?.abort(), []);
  const retain = (intent?: Intent) => {
    try {
      if (!owner) throw new Error();
      if (intent)
        sessionStorage.setItem(
          lifecycleStorageKey(owner, workspaceId),
          JSON.stringify({ version: 1, intent }),
        );
      else sessionStorage.removeItem(lifecycleStorageKey(owner, workspaceId));
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
    const poll = async () => {
      try {
        const value = await client.readAgentLifecycle!(receipt.id, { signal: abort.signal });
        if (abort.signal.aborted) return;
        setOperation(value);
        setFailure(undefined);
        if (value.status === 'waiting_for_connection') setConnection(value.connection);
        if (value.status === 'queued' || value.status === 'running')
          timer = setTimeout(() => void poll(), 1000);
      } catch (error: unknown) {
        if (!abort.signal.aborted)
          setFailure(error instanceof Error ? error.message : 'Status unavailable.');
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
    blocked: saved.error !== undefined,
  };
}
