import { useEffect, useRef, useState } from 'react';
import { ConnectionWizard } from '../connections/ConnectionWizard';
import { connectionClient } from '../connections/client';
import { Button } from '../ui/Button';
import { Modal } from '../ui/Modal';
import { ScopePicker } from './ScopePicker';
import { REQUIRED_AGENT_SCOPES } from './SettingsFields';
import { accessRequests, validAccessServices, type AgentAccessRequest } from './accessRequests';
import { loadAccessDecision, saveAccessDecision } from './accessDecisionStorage';

export function AgentAccessRequests({
  agentId,
  owner,
  granted,
  enabled,
}: {
  readonly agentId: string;
  readonly owner?: string;
  readonly granted: readonly string[];
  readonly enabled: boolean;
}) {
  const [requests, setRequests] = useState<readonly AgentAccessRequest[]>([]);
  const [error, setError] = useState<string>();
  const [refresh, setRefresh] = useState(0);
  const [dismissed, setDismissed] = useState<readonly string[]>([]);
  useEffect(() => {
    if (!enabled) return;
    const request = new AbortController();
    let timer: ReturnType<typeof setTimeout> | undefined;
    let inFlight = false;
    const poll = async () => {
      if (request.signal.aborted || inFlight || document.hidden) return;
      clearTimeout(timer);
      inFlight = true;
      try {
        const next = await accessRequests.list(agentId, request.signal);
        if (request.signal.aborted) return;
        setRequests(next);
        setError(undefined);
      } catch (cause) {
        if (request.signal.aborted) return;
        setError(
          cause instanceof Error ? cause.message : 'Agent access requests could not be checked.',
        );
      } finally {
        inFlight = false;
      }
      if (!request.signal.aborted) timer = setTimeout(() => void poll(), 2000);
    };
    const onVisibilityChange = () => void poll();
    document.addEventListener('visibilitychange', onVisibilityChange);
    void poll();
    return () => {
      request.abort();
      clearTimeout(timer);
      document.removeEventListener('visibilitychange', onVisibilityChange);
    };
  }, [agentId, enabled, refresh]);
  if (!enabled) return null;
  const request =
    requests.find((item) => ['pending', 'connecting', 'applying'].includes(item.status)) ??
    requests.find((item) => item.status === 'failed' && !dismissed.includes(item.id));
  return (
    <>
      {error ? (
        <p className="agent-action-error" role="status">
          {error}
        </p>
      ) : null}
      {!request ? null : dismissed.includes(request.id) ? (
        <div className="agent-action-error" role="status">
          <span>Your Agent requested additional service access.</span>
          <Button onClick={() => setDismissed((ids) => ids.filter((id) => id !== request.id))}>
            Review access request
          </Button>
        </div>
      ) : (
        <AccessRequestDialog
          key={request.id}
          request={request}
          owner={owner}
          granted={granted}
          onUpdate={(value) =>
            setRequests((items) => items.map((item) => (item.id === value.id ? value : item)))
          }
          onRefresh={() => setRefresh((value) => value + 1)}
          onClose={() => setDismissed((ids) => [...ids, request.id])}
        />
      )}
    </>
  );
}

function AccessRequestDialog({
  request,
  owner,
  granted,
  onUpdate,
  onRefresh,
  onClose,
}: {
  readonly request: AgentAccessRequest;
  readonly owner?: string;
  readonly granted: readonly string[];
  readonly onUpdate: (request: AgentAccessRequest) => void;
  readonly onRefresh: () => void;
  readonly onClose: () => void;
}) {
  const [saved, setSaved] = useState(() => loadAccessDecision(owner, request.agent_id, request.id));
  const intent = saved.intent;
  const [services, setServices] = useState<readonly string[]>(
    intent?.services ?? [...new Set([...REQUIRED_AGENT_SCOPES, ...granted, ...request.services])],
  );
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string>();
  const active = useRef<AbortController | undefined>(undefined);
  const [connected, setConnected] = useState(false);
  const connectionTargetId =
    request.status === 'connecting' || request.status === 'applying'
      ? request.connection?.target.id
      : undefined;
  useEffect(() => () => active.current?.abort(), []);
  useEffect(() => {
    if (!connected || !connectionTargetId) return;
    const controller = new AbortController();
    let timer: ReturnType<typeof setTimeout> | undefined;
    const check = async () => {
      try {
        const report = await connectionClient.check(
          { type: 'agent_access_request', id: connectionTargetId },
          undefined,
          controller.signal,
        );
        if (controller.signal.aborted) return;
        if (!report.ready || (owner !== undefined && report.owner !== owner)) {
          setConnected(false);
          return;
        }
      } catch {
        if (controller.signal.aborted) return;
        // The wizard rechecks authority and presents connection or sign-in failures.
        setConnected(false);
        return;
      }
      timer = setTimeout(() => void check(), 2000);
    };
    // A ready connection can expire while the durable workflow is still applying.
    timer = setTimeout(() => void check(), 2000);
    return () => {
      controller.abort();
      clearTimeout(timer);
    };
  }, [connected, connectionTargetId, owner]);
  const decide = async (action: 'approve' | 'deny') => {
    if (active.current || saved.error) return;
    const selected = intent ?? {
      key: crypto.randomUUID(),
      action,
      services: action === 'approve' ? services : [],
    };
    if (selected.action === 'approve' && !validAccessServices(selected.services)) {
      setError('Choose up to 32 services. Existing Sandbox access must remain included.');
      return;
    }
    try {
      saveAccessDecision(owner, request.agent_id, request.id, selected);
      setSaved({ intent: selected });
    } catch {
      setError(
        'The access decision could not be saved. Restore browser session storage before retrying.',
      );
      return;
    }
    const abort = new AbortController();
    active.current = abort;
    setPending(true);
    setError(undefined);
    try {
      const value = await accessRequests.decide(
        request.agent_id,
        request.id,
        selected.action,
        selected.key,
        selected.services,
        abort.signal,
      );
      if (!abort.signal.aborted) {
        saveAccessDecision(owner, request.agent_id, request.id);
        setSaved({});
        onUpdate(value);
        onRefresh();
      }
    } catch (cause) {
      if (!abort.signal.aborted)
        setError(
          cause instanceof Error
            ? cause.message
            : 'The decision could not be confirmed. Retry the same request.',
        );
    } finally {
      active.current = undefined;
      if (!abort.signal.aborted) setPending(false);
    }
  };
  if (connectionTargetId && request.connection && !connected)
    return (
      <ConnectionWizard
        target={request.connection.target}
        expectedOwner={owner}
        presentation="dialog"
        dialogTitle="Connect Agent services"
        onCancel={onClose}
        onReady={() => {
          setConnected(true);
          onRefresh();
        }}
      />
    );
  return (
    <Modal
      open
      title="Agent key access"
      onClose={onClose}
      dismissible={!pending}
      footer={
        request.status === 'pending' ? (
          <>
            <Button
              disabled={pending || !!intent || !!saved.error}
              onClick={() => void decide('deny')}
            >
              Deny
            </Button>
            <Button
              variant="primary"
              disabled={pending || !!saved.error}
              onClick={() => void decide('approve')}
            >
              {pending
                ? 'Saving decision…'
                : intent
                  ? 'Retry access decision'
                  : 'Approve and connect'}
            </Button>
          </>
        ) : (
          <Button onClick={onClose}>Close</Button>
        )
      }
    >
      <p>Your Agent requested service access to continue its task.</p>
      <p>{request.reason}</p>
      <p>
        Approving briefly pauses and resumes this shared Sandbox to update its Agent keys. Other
        Agents keep their conversation history.
      </p>
      {request.status === 'pending' ? (
        <ScopePicker
          value={services}
          onChange={setServices}
          fixed={granted}
          requested={request.services}
          disabled={pending || !!intent || !!saved.error}
        />
      ) : (
        <ul>
          {services.map((service) => (
            <li key={service}>{service}</li>
          ))}
        </ul>
      )}
      {request.status === 'connecting' || request.status === 'applying' ? (
        <p role="status">
          Verifying the updated Agent key in the runtime. The original Agent will be notified
          automatically after verification.
        </p>
      ) : null}
      {request.status === 'failed' ? (
        <p role="alert">
          {request.error ??
            'Access could not be applied. The Agent has not been notified that access is ready.'}
        </p>
      ) : null}
      {saved.error ? <p role="alert">{saved.error}</p> : null}
      {error ? <p role="alert">{error}</p> : null}
    </Modal>
  );
}
