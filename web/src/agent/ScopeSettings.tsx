import { observeRead } from './transport/readObserver';
import { retrySchedule } from './transport/readRetry';
import { useEffect, useRef, useState } from 'react';
import { useCmaClient } from '../api/context';
import type { AgentDetail } from '../api/types';
import { ConnectionWizard } from '../connections/ConnectionWizard';
import { Button } from '../ui/Button';
import { ScopePicker } from './ScopePicker';
import { REQUIRED_AGENT_SCOPES } from './requiredScopes';
import { accessRequests, AgentAccessError, type AgentAccessRequest } from './accessRequests';
import { useScopeConnections } from './scopeConnections';
import { loadScopeUpdate, saveScopeUpdate, type ScopeUpdateIntent } from './scopeUpdateStorage';

const scopes = (value: readonly string[]) =>
  [...new Set([...REQUIRED_AGENT_SCOPES, ...value])].sort();
const same = (left: readonly string[], right: readonly string[]) =>
  JSON.stringify([...left].sort()) === JSON.stringify([...right].sort());

export function ScopeSettings({
  detail,
  owner,
  active = true,
}: {
  readonly detail: AgentDetail;
  readonly owner?: string;
  readonly active?: boolean;
}) {
  const client = useCmaClient();
  const [saved, setSaved] = useState(() => loadScopeUpdate(owner, detail.id));
  const intent = saved.intent;
  const [granted, setGranted] = useState(() => scopes(detail.reach));
  const [selected, setSelected] = useState<readonly string[]>(intent?.services ?? granted);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string>();
  const [result, setResult] = useState<AgentAccessRequest>();
  const [conflict, setConflict] = useState(false);
  const [connecting, setConnecting] = useState(false);
  const inFlight = useRef<AbortController | undefined>(undefined);
  const changed = !same(selected, granted);
  const connections = useScopeConnections(detail.id, owner, selected, active && changed && !intent);
  useEffect(() => () => inFlight.current?.abort(), []);

  const remember = (value?: ScopeUpdateIntent) => {
    if (!owner) throw new Error('Your account could not be verified. Open the Agent again.');
    saveScopeUpdate(owner, detail.id, value);
    setSaved(value ? { intent: value } : {});
  };
  const observe = (request: AgentAccessRequest) => {
    setResult(request);
    if (request.status === 'succeeded') {
      remember();
      setGranted([...request.services].sort());
      setSelected([...request.services].sort());
    }
  };
  const observeRef = useRef(observe);
  useEffect(() => {
    observeRef.current = observe;
  });
  useEffect(() => {
    if (!active || !intent?.requestId || result?.status === 'failed') return;
    const started = performance.now();
    let failures = 0;
    const observer = observeRead(async (signal) => {
      try {
        const requests = await accessRequests.list(detail.id, signal);
        if (signal.aborted) return;
        const request = requests.find(({ id }) => id === intent.requestId);
        if (request) {
          observeRef.current(request);
          if (['succeeded', 'failed', 'denied'].includes(request.status)) return;
        }
        setError(undefined);
        failures = 0;
      } catch (cause) {
        if (signal.aborted) return;
        setError(cause instanceof Error ? cause.message : 'The update could not be checked.');
        return retrySchedule(cause, ++failures);
      }
      const next = performance.now() - started < 5000 ? 500 : 2000;
      return { next, cooldown: next };
    });
    return () => observer.dispose();
  }, [active, detail.id, intent?.requestId, result?.status]);

  const save = async () => {
    if (inFlight.current || !owner || saved.error || (!intent && !connections.ready)) return;
    const retained = intent ?? {
      key: crypto.randomUUID(),
      services: selected,
      expectedServices: granted,
    };
    if (retained.requestId) return;
    try {
      remember(retained);
    } catch {
      setError(
        'The scope update could not be saved. Restore browser session storage before retrying.',
      );
      return;
    }
    const controller = new AbortController();
    inFlight.current = controller;
    setPending(true);
    setError(undefined);
    try {
      const request = await accessRequests.replace(
        detail.id,
        retained.services,
        retained.expectedServices,
        retained.key,
        controller.signal,
      );
      if (controller.signal.aborted) return;
      remember({ ...retained, requestId: request.id });
      observe(request);
    } catch (cause) {
      if (!controller.signal.aborted) {
        setConflict(cause instanceof AgentAccessError && cause.status === 409);
        setError(
          cause instanceof Error
            ? cause.message
            : 'The update could not be confirmed. Retry the same request.',
        );
      }
    } finally {
      inFlight.current = undefined;
      if (!controller.signal.aborted) setPending(false);
    }
  };
  const reload = async () => {
    if (inFlight.current) return;
    const controller = new AbortController();
    inFlight.current = controller;
    setPending(true);
    try {
      const current = await client.readAgent(detail.id, { signal: controller.signal });
      if (controller.signal.aborted) return;
      remember();
      setGranted(scopes(current.reach));
      setSelected(scopes(current.reach));
      setResult(undefined);
      setConflict(false);
      setError(undefined);
    } catch (cause) {
      if (!controller.signal.aborted)
        setError(cause instanceof Error ? cause.message : 'Current access could not be checked.');
    } finally {
      inFlight.current = undefined;
      if (!controller.signal.aborted) setPending(false);
    }
  };
  return (
    <div className="agent-settings">
      <ScopePicker
        value={selected}
        onChange={(value) => {
          setSelected(value);
          setResult(undefined);
        }}
        replace
        disabled={pending || !!intent || !!saved.error || !owner}
      />
      <p>Save updates the shared key for every Agent in this Sandbox without restarting them.</p>
      {detail.sandbox.status === 'paused' ? (
        <p>The Sandbox is paused. Saved access will be applied when you resume it.</p>
      ) : null}
      {connections.checking ? <p role="status">Checking service connections…</p> : null}
      {connections.error ? (
        <>
          <p role="alert">{connections.error}</p>
          <Button onClick={connections.retry}>Retry connection check</Button>
        </>
      ) : null}
      {connections.needsConnection ? (
        <>
          {connections.missingServices?.length ? (
            <p role="status">
              Verify connections for{' '}
              {connections.missingServices.map((service) => service.display_name).join(', ')} before
              saving.
            </p>
          ) : null}
          <Button onClick={() => setConnecting(true)}>Connect services before saving</Button>
          <Button onClick={connections.retry}>Check connections again</Button>
        </>
      ) : null}
      {intent?.requestId && result?.connection ? (
        <Button onClick={() => setConnecting(true)}>Review service connections</Button>
      ) : null}
      {connecting ? (
        <ConnectionWizard
          target={result?.connection?.target ?? connections.target}
          expectedOwner={owner}
          presentation="dialog"
          description="Connect the selected services, then return here to save the Sandbox scope."
          onCancel={() => setConnecting(false)}
          onReady={(report) => {
            connections.connected(report);
            setConnecting(false);
          }}
        />
      ) : null}
      {saved.error || error ? <p role="alert">{saved.error ?? error}</p> : null}
      {result?.status === 'failed' || conflict ? (
        <>
          <p role="alert">
            {result?.error ??
              'Access could not be updated. Reload current access before trying again.'}
          </p>
          <Button disabled={pending} onClick={() => void reload()}>
            Reload current scope
          </Button>
        </>
      ) : null}
      {result?.status === 'succeeded' ? (
        <p role="status">Agent key updated. Your Agent has been notified.</p>
      ) : null}
      {intent?.requestId && result?.status !== 'failed' ? (
        <p role="status">Updating the key and notifying your Agent…</p>
      ) : null}
      <Button
        variant="primary"
        disabled={
          pending ||
          !owner ||
          !!saved.error ||
          !!intent?.requestId ||
          conflict ||
          (!intent && (!changed || !connections.ready))
        }
        onClick={() => void save()}
      >
        {pending
          ? 'Saving scope…'
          : intent && !intent.requestId
            ? 'Retry scope update'
            : 'Save scope'}
      </Button>
    </div>
  );
}
