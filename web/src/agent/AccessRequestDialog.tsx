import { useEffect, useMemo, useRef, useState } from 'react';
import { Alert } from '../ui/Alert';
import { ConnectionWizard } from '../connections/ConnectionWizard';
import { GitHubConnectionDetails } from '../connections/RepositoryConnectionRepair';
import { Button } from '../ui/Button';
import { Modal } from '../ui/Modal';
import { ScopePicker } from './ScopePicker';
import { REQUIRED_AGENT_SCOPES } from './requiredScopes';
import {
  accessRequests,
  AgentAccessError,
  validAccessServices,
  type AgentAccessRequest,
  type GithubConnectionApproval,
} from './accessRequests';
import { loadAccessDecision, saveAccessDecision } from './accessDecisionStorage';
import type { SandboxStatus } from '../api/types';
import { useScopeConnections } from './scopeConnections';
import { useAccessRequests } from './useAccessRequests';

export function AgentAccessRequests({
  agentId,
  owner,
  granted,
  enabled,
  sandboxStatus,
  running = false,
  eventRevision = '',
}: {
  readonly agentId: string;
  readonly owner?: string;
  readonly granted: readonly string[];
  readonly enabled: boolean;
  readonly sandboxStatus?: SandboxStatus;
  readonly running?: boolean;
  readonly eventRevision?: string | number;
}) {
  const { requests, error, refresh, update } = useAccessRequests(
    agentId,
    owner,
    enabled,
    running,
    eventRevision,
  );
  const [dismissed, setDismissed] = useState<readonly string[]>([]);
  if (!enabled) return null;
  const request =
    requests.find((item) => ['pending', 'connecting', 'applying'].includes(item.status)) ??
    requests.find((item) => item.status === 'failed' && !dismissed.includes(item.id));
  return (
    <>
      {error ? <Alert tone="warning">{error}</Alert> : null}
      {!request ? null : dismissed.includes(request.id) ? (
        <Alert
          tone="info"
          actions={
            <Button
              size="sm"
              onClick={() => setDismissed((ids) => ids.filter((id) => id !== request.id))}
            >
              Review access request
            </Button>
          }
        >
          Your Agent requested additional service access.
        </Alert>
      ) : (
        <AccessRequestDialog
          key={`${owner}:${agentId}:${request.id}`}
          request={request}
          owner={owner}
          granted={granted}
          sandboxStatus={sandboxStatus}
          onUpdate={update}
          onRefresh={refresh}
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
  sandboxStatus,
  onUpdate,
  onRefresh,
  onClose,
}: {
  readonly request: AgentAccessRequest;
  readonly owner?: string;
  readonly granted: readonly string[];
  readonly sandboxStatus?: SandboxStatus;
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
  const [preparingConnections, setPreparingConnections] = useState(false);
  const [showGitHubConnections, setShowGitHubConnections] = useState(false);
  const [selection, setSelection] = useState<GithubConnectionApproval>();
  const approval = request.github_approval;
  const [refusedBinding, setRefusedBinding] = useState<GithubConnectionApproval>();
  const awaitingCurrentBinding =
    refusedBinding !== undefined &&
    (!approval ||
      (refusedBinding.sandbox_id === approval.sandbox_id &&
        refusedBinding.scope_revision === approval.scope_revision &&
        refusedBinding.credential_generation === approval.credential_generation));
  const selectedGithub =
    !awaitingCurrentBinding &&
    selection &&
    approval &&
    selection.sandbox_id === approval.sandbox_id &&
    selection.scope_revision === approval.scope_revision &&
    selection.credential_generation === approval.credential_generation
      ? selection
      : undefined;
  const preflight = useScopeConnections(
    request.agent_id,
    owner,
    approval ? services.filter((service) => service !== 'api-github') : services,
    request.status === 'pending' && !intent,
  );
  const githubInventoryTarget = useMemo(
    () => ({ type: 'agent_service_access' as const, id: request.agent_id, services }),
    [request.agent_id, services],
  );
  const connectionTargetId =
    request.status === 'connecting' || request.status === 'applying'
      ? request.connection?.target.id
      : undefined;
  useEffect(() => () => active.current?.abort(), []);
  const decide = async (action: 'approve' | 'deny') => {
    if (
      active.current ||
      saved.error ||
      (action === 'approve' &&
        !intent &&
        (awaitingCurrentBinding || !preflight.ready || (approval && !selectedGithub)))
    )
      return;
    const selected = intent ?? {
      key: crypto.randomUUID(),
      action,
      services: action === 'approve' ? services : [],
      githubConnection: action === 'approve' ? selectedGithub : undefined,
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
        selected.githubConnection,
      );
      if (!abort.signal.aborted) {
        saveAccessDecision(owner, request.agent_id, request.id);
        setSaved({});
        if (selected.action === 'approve' && preflight.ready) setConnected(true);
        onUpdate(value);
        onRefresh();
      }
    } catch (cause) {
      if (!abort.signal.aborted) {
        if (cause instanceof AgentAccessError && cause.status === 409) {
          if (cause.code === 'agent_access_stale_github_approval' && selected.githubConnection) {
            // The transaction refused before saving the decision. Unlock only
            // this exact intent, and require a fresh choice on current context.
            try {
              saveAccessDecision(owner, request.agent_id, request.id);
              setSaved({});
              setSelection(undefined);
              setRefusedBinding(selected.githubConnection);
            } catch {
              setError(
                'The saved decision could not be cleared. Restore browser session storage before retrying.',
              );
              return;
            }
          }
          // A generic conflict may represent a different committed decision.
          // Keep its exact retry intent while refreshing the durable request.
          onRefresh();
        }
        setError(
          cause instanceof Error
            ? cause.message
            : 'The decision could not be confirmed. Retry the same request.',
        );
      }
    } finally {
      active.current = undefined;
      if (!abort.signal.aborted) setPending(false);
    }
  };
  if (preparingConnections)
    return (
      <ConnectionWizard
        target={preflight.target}
        expectedOwner={owner}
        presentation="dialog"
        dialogTitle="Connect Agent services"
        onCancel={() => setPreparingConnections(false)}
        onReady={(report) => {
          preflight.connected(report);
          setPreparingConnections(false);
        }}
      />
    );
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
              disabled={
                pending ||
                !!saved.error ||
                (!intent &&
                  (awaitingCurrentBinding ||
                    !preflight.ready ||
                    (approval !== undefined && !selectedGithub)))
              }
              onClick={() => void decide('approve')}
            >
              {pending ? 'Saving decision…' : intent ? 'Retry access decision' : 'Approve'}
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
        Approving updates service access without restarting this Sandbox or interrupting its Agents.
        If the Sandbox is already paused, the request waits until you resume it.
      </p>
      {request.services.includes('api-github') ? (
        <>
          {approval ? (
            <>
              <p>
                This choice changes GitHub access for Sandbox {approval.sandbox_name} (
                {approval.sandbox_id}).
              </p>
              {approval.other_agents.length > 0 ? (
                <>
                  <p>Other Agents sharing this credential</p>
                  <ul>
                    {approval.other_agents.map((id) => (
                      <li key={id}>{id}</li>
                    ))}
                  </ul>
                </>
              ) : null}
              {approval.current_user_service_id ? (
                <p>Currently selected connection: {approval.current_user_service_id}</p>
              ) : null}
            </>
          ) : (
            <p>GitHub connection selection is separate from service approval.</p>
          )}
          {owner && approval ? (
            <GitHubConnectionDetails
              owner={owner}
              target={githubInventoryTarget}
              selection={{
                id: (intent?.githubConnection ?? selectedGithub)?.user_service_id,
                disabled: pending || !!intent || !!saved.error || awaitingCurrentBinding,
                onSelect: (id) =>
                  setSelection({
                    sandbox_id: approval.sandbox_id,
                    user_service_id: id,
                    scope_revision: approval.scope_revision,
                    credential_generation: approval.credential_generation,
                  }),
              }}
            />
          ) : owner && services.includes('api-github') ? (
            <>
              <Button
                size="sm"
                aria-expanded={showGitHubConnections}
                onClick={() => setShowGitHubConnections((show) => !show)}
              >
                {showGitHubConnections ? 'Hide GitHub connections' : 'Manage GitHub connections'}
              </Button>
              {showGitHubConnections ? (
                <GitHubConnectionDetails owner={owner} target={preflight.target} />
              ) : null}
            </>
          ) : null}
        </>
      ) : null}
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
          {sandboxStatus === 'paused'
            ? 'The Sandbox is paused. Your approval is saved; CMA will notify the Agent after the Sandbox is running and the updated key is verified.'
            : sandboxStatus === 'pausing'
              ? 'Waiting for the Sandbox to finish pausing before continuing the access update.'
              : 'Verifying the updated Agent key in the runtime. The original Agent will be notified automatically after verification.'}
        </p>
      ) : null}
      {request.status === 'failed' ? (
        <p role="alert">
          {request.error === 'access_update_cannot_continue'
            ? 'This access update could not be completed. Ask the Agent to create a new access request. Repeating this failed request does not restart it.'
            : (request.error ??
              'Access could not be applied. The Agent has not been notified that access is ready.')}
        </p>
      ) : null}
      {connectionTargetId && connected && request.connection ? (
        <Button onClick={() => setConnected(false)}>Review service connections</Button>
      ) : null}
      {preflight.checking ? <p role="status">Checking service connections…</p> : null}
      {preflight.needsConnection ? (
        <>
          <p>Connect the selected services before approving access.</p>
          <Button onClick={() => setPreparingConnections(true)}>Connect services</Button>
        </>
      ) : null}
      {preflight.error ? (
        <>
          <p role="alert">{preflight.error}</p>
          <Button onClick={preflight.retry}>Retry connection check</Button>
        </>
      ) : null}
      {saved.error ? <p role="alert">{saved.error}</p> : null}
      {error ? <p role="alert">{error}</p> : null}
    </Modal>
  );
}
