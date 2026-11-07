import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import {
  ApiError,
  type AgentBrowserControlClient,
  type BrowserControl as Control,
  type BrowserControlChange,
  type BrowserInput,
} from '@chronoai/cma-sdk';
import { Button } from '../../ui/Button';
import { controlRecovery } from './browserControlRecovery';

type Phase = 'loading' | 'ready' | 'transferring' | 'unknown' | 'forbidden';
type View = { value?: Control; phase: Phase; token?: string; pending?: BrowserControlChange };
const keys = new Set([
  'Backspace',
  'Tab',
  'Enter',
  'Escape',
  'Home',
  'End',
  'ArrowLeft',
  'ArrowUp',
  'ArrowRight',
  'ArrowDown',
  'Delete',
]);

export function BrowserControl({
  client,
  accountId,
  agentId,
  incarnation,
  contextId,
  active,
  running = false,
  admissionAllowed = false,
  children,
}: {
  readonly client: AgentBrowserControlClient;
  readonly accountId: string;
  readonly agentId: string;
  readonly incarnation: string;
  readonly contextId: string;
  readonly active: boolean;
  readonly running?: boolean;
  readonly admissionAllowed?: boolean;
  readonly children: ReactNode;
}) {
  const binding = useMemo(
    () => ({
      client,
      agentId,
      scope: {
        runtime_incarnation: incarnation,
        context_id: contextId,
        client_id: crypto.randomUUID(),
        client_key: crypto.randomUUID(),
      },
      recovery: controlRecovery(accountId, agentId, incarnation, contextId),
    }),
    [client, accountId, agentId, incarnation, contextId],
  );
  const { scope, recovery } = binding;
  const lifetime = useRef({ alive: true });
  const current = useRef<View>({ phase: 'loading' });
  const busy = useRef(false);
  const ready = useRef(active);
  const admission = useRef(admissionAllowed);
  const inputs = useRef<BrowserInput[]>([]);
  const draining = useRef(false);
  const renewAt = useRef(0);
  const [view, setView] = useState<View>({ phase: 'loading' });
  const [sending, setSending] = useState(false);
  const [text, setText] = useState('');
  const [notice, setNotice] = useState('');
  const [clock, setClock] = useState(Date.now);
  const visible = () => document.visibilityState !== 'hidden' && navigator.onLine;
  const publish = (value: View) => {
    current.current = value;
    setView(value);
  };
  const accept = (value: Control) => {
    const own = value.state === 'user' && value.client_id === scope.client_id;
    publish({
      phase: value.state === 'unknown' || !visible() ? 'unknown' : 'ready',
      value,
      token: own ? (value.lease_token ?? current.current.token) : undefined,
    });
    renewAt.current = Date.now() + 5000;
  };
  const fail = (error: unknown, pending?: BrowserControlChange) => {
    inputs.current = [];
    const forbidden = error instanceof ApiError && [401, 403, 404].includes(error.status);
    publish({
      ...current.current,
      phase: forbidden ? 'forbidden' : 'unknown',
      pending,
      token: forbidden ? undefined : current.current.token,
    });
  };
  const execute = async (request: BrowserControlChange) => {
    if (busy.current || !lifetime.current.alive) return false;
    const version = lifetime.current;
    busy.current = true;
    setSending(true);
    recovery.save(request);
    publish({
      ...current.current,
      pending: request,
      phase: ['acquire', 'return'].includes(request.command)
        ? 'transferring'
        : current.current.phase,
    });
    try {
      const value = await client.change(agentId, request);
      recovery.clear(request.operation_id);
      if (!version.alive) return false;
      accept(value);
      return true;
    } catch (error) {
      if (version.alive) fail(error, request);
      return false;
    } finally {
      if (version.alive) {
        busy.current = false;
        setSending(false);
        queueMicrotask(() => actions.current.drain());
      }
    }
  };
  const command = (command: BrowserControlChange['command'], input?: BrowserInput) => {
    if (command !== 'return' && !admission.current) return Promise.resolve(false);
    const value = current.current;
    if (
      (command !== 'return' && !ready.current) ||
      !visible() ||
      value.phase !== 'ready' ||
      !value.value
    )
      return Promise.resolve(false);
    if (
      command !== 'acquire' &&
      (!value.token ||
        value.value.client_id !== scope.client_id ||
        Date.parse(value.value.expires_at ?? '') <= Date.now())
    )
      return Promise.resolve(false);
    return execute({
      ...scope,
      command,
      operation_id: crypto.randomUUID(),
      expected_revision: value.value.revision,
      ...(command === 'acquire' ? {} : { lease_token: value.token }),
      ...(input ? { input } : {}),
    });
  };
  const read = async () => {
    if (busy.current || !lifetime.current.alive || !visible()) return;
    const version = lifetime.current;
    busy.current = true;
    setSending(true);
    try {
      const pending = current.current.pending ?? recovery.read();
      const value = await client.read(
        agentId,
        pending
          ? {
              runtime_incarnation: pending.runtime_incarnation,
              context_id: pending.context_id,
              client_id: pending.client_id,
              client_key: pending.client_key,
              operation_id: pending.operation_id,
            }
          : scope,
      );
      recovery.clear(pending?.operation_id);
      if (version.alive) accept(value);
    } catch (error) {
      if (version.alive) fail(error, current.current.pending);
    } finally {
      if (version.alive) {
        busy.current = false;
        setSending(false);
        queueMicrotask(() => actions.current.drain());
      }
    }
  };
  const drain = async () => {
    if (busy.current || draining.current) return;
    draining.current = true;
    try {
      while (inputs.current.length) {
        const input = inputs.current.shift()!;
        if (!(await command('input', input))) {
          inputs.current = [];
          break;
        }
      }
    } finally {
      draining.current = false;
    }
  };
  const actions = useRef({ read, command, drain });
  useEffect(() => {
    actions.current = { read, command, drain };
    ready.current = active;
    admission.current = admissionAllowed;
    if (!admissionAllowed) inputs.current = [];
  });
  useEffect(() => {
    const version = { alive: true };
    lifetime.current = version;
    current.current = { phase: 'loading' };
    busy.current = false;
    inputs.current = [];
    queueMicrotask(() => {
      if (version.alive) void actions.current.read();
    });
    const timer = setInterval(() => {
      setClock(Date.now());
      if (!ready.current || !visible() || busy.current) return;
      const value = current.current;
      if (value.phase !== 'ready') return;
      if (value.token && Date.parse(value.value?.expires_at ?? '') <= Date.now()) {
        inputs.current = [];
        publish({ ...value, phase: 'unknown', token: undefined });
        return;
      }
      if (value.token && Date.now() >= renewAt.current) void actions.current.command('renew');
      else if (!value.token) void actions.current.read();
    }, 1000);
    const reconnect = () => {
      if (!visible()) {
        inputs.current = [];
        publish({ ...current.current, phase: 'unknown' });
      } else void actions.current.read();
    };
    document.addEventListener('visibilitychange', reconnect);
    window.addEventListener('offline', reconnect);
    window.addEventListener('online', reconnect);
    return () => {
      version.alive = false;
      clearInterval(timer);
      document.removeEventListener('visibilitychange', reconnect);
      window.removeEventListener('offline', reconnect);
      window.removeEventListener('online', reconnect);
      const value = current.current;
      if (value.token && value.value && !busy.current && value.phase === 'ready') {
        const request: BrowserControlChange = {
          ...scope,
          command: 'return',
          operation_id: crypto.randomUUID(),
          expected_revision: value.value.revision,
          lease_token: value.token,
        };
        recovery.save(request);
        void client
          .change(agentId, request)
          .then(() => recovery.clear(request.operation_id))
          .catch(() => {});
      }
    };
  }, [client, agentId, scope, recovery]);
  const owned =
    view.phase === 'ready' &&
    view.value?.state === 'user' &&
    view.value.client_id === scope.client_id &&
    !!view.token &&
    Date.parse(view.value.expires_at ?? '') > clock;
  const canInput = owned && active && admissionAllowed && visible();
  const enqueue = async (input: BrowserInput) => {
    if (!canInput) return;
    if (inputs.current.length >= 32) {
      setNotice('Input is busy. Wait for confirmation before typing more.');
      return;
    }
    inputs.current.push(input);
    await drain();
  };
  const label =
    view.phase === 'transferring'
      ? 'Transferring control…'
      : view.phase === 'unknown' ||
          (view.value?.state === 'user' && Date.parse(view.value.expires_at ?? '') <= clock)
        ? 'Control state unknown'
        : view.phase === 'forbidden'
          ? 'Browser control is not permitted.'
          : view.phase === 'loading'
            ? 'Checking browser control…'
            : owned
              ? 'You are controlling'
              : view.value?.state === 'user'
                ? 'Another client is controlling'
                : running
                  ? 'Agent is in control · Working in Browser'
                  : 'Agent paused · Desktop is idle';
  return (
    <>
      <div className="agent-browser__control">
        <p className="workspace-control-status" role="status">
          <span
            className={`workspace-control-dot${running && !owned ? ' workspace-control-dot--running' : ''}`}
            aria-hidden="true"
          />
          {label}
        </p>
        {view.phase === 'ready' && view.value?.state === 'agent' ? (
          <Button
            size="sm"
            variant="primary"
            placement="workspace-take-control"
            disabled={!active || !admissionAllowed || sending}
            onClick={() => {
              void command('acquire');
            }}
          >
            Take control
          </Button>
        ) : null}
        {owned ? (
          <Button
            size="sm"
            disabled={sending}
            onClick={() => {
              inputs.current = [];
              void command('return');
            }}
          >
            Return to Agent
          </Button>
        ) : null}
        {view.phase === 'unknown' || (view.phase === 'forbidden' && view.pending) ? (
          <>
            <Button
              size="sm"
              disabled={sending}
              onClick={() => {
                void read();
              }}
            >
              Check original operation
            </Button>
            {view.pending ? (
              <Button
                size="sm"
                disabled={sending || (!admissionAllowed && view.pending.command !== 'return')}
                onClick={() => {
                  if (view.pending) void execute(view.pending);
                }}
              >
                Retry exact operation
              </Button>
            ) : null}
          </>
        ) : null}
      </div>
      <div
        role="application"
        aria-label="Agent browser input"
        tabIndex={canInput ? 0 : -1}
        aria-disabled={!canInput}
        className="agent-browser__input"
        onClick={(event) => {
          if (!(event.target instanceof HTMLImageElement) || !canInput) return;
          const rectangle = event.target.getBoundingClientRect();
          if (!rectangle.width || !rectangle.height) return;
          event.currentTarget.focus();
          void enqueue({
            kind: 'click',
            x: Math.floor(((event.clientX - rectangle.left) * 1280) / rectangle.width),
            y: Math.floor(((event.clientY - rectangle.top) * 720) / rectangle.height),
          });
        }}
        onKeyDown={(event) => {
          if (
            !canInput ||
            event.ctrlKey ||
            event.metaKey ||
            event.altKey ||
            event.nativeEvent.isComposing ||
            event.key === 'Tab'
          )
            return;
          if (event.key.length === 1) {
            event.preventDefault();
            void enqueue({ kind: 'text', text: event.key });
          } else if (keys.has(event.key)) {
            event.preventDefault();
            void enqueue({
              kind: 'key',
              key: event.key as Extract<BrowserInput, { kind: 'key' }>['key'],
            });
          }
        }}
      >
        {children}
      </div>
      {owned ? (
        <div className="agent-browser__control">
          <label>
            Text to type in browser
            <input
              value={text}
              maxLength={1024}
              disabled={!canInput || sending}
              onChange={(event) => setText(event.target.value)}
            />
          </label>
          <Button
            size="sm"
            disabled={!canInput || sending || !text}
            onClick={() => {
              void command('input', { kind: 'text', text }).then((accepted) => {
                if (accepted) setText('');
              });
            }}
          >
            Type text
          </Button>
        </div>
      ) : null}
      {notice ? <p role="alert">{notice}</p> : null}
      {view.value ? (
        <details className="agent-browser__audit">
          <summary>Control log</summary>
          {view.value.audit_truncated_before !== null ? (
            <p>Earlier control records are no longer retained.</p>
          ) : null}
          <ol>
            {view.value.audit.map((entry) => (
              <li key={entry.revision}>
                {entry.kind} ·{' '}
                <time dateTime={entry.at}>{new Date(entry.at).toLocaleTimeString()}</time>
              </li>
            ))}
          </ol>
        </details>
      ) : null}
    </>
  );
}
