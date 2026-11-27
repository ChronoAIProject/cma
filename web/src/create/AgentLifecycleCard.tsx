import { useSandboxCapacity } from '../sandbox/useSandboxCapacity';
import { SandboxCapacityPanel } from '../sandbox/SandboxCapacityPanel';
import { useEffect, useMemo, useRef, useState } from 'react';
import type {
  AgentLifecycleCreateInput,
  AgentLifecycleCreateReceipt,
  CmaWebClient,
} from '../api/client';
import { ConnectionWizard, RepositoryConnectionRepair } from '../connections/ConnectionWizard';
import { Button } from '../ui/Button';
import { Input } from '../ui/Input';
import { IconAgentPresetOutline16 } from '../ui/icons';
import { Modal } from '../ui/Modal';
import { useLifecycleIntent } from './lifecycleIntent';
import { lifecycleFailureMessage } from './lifecycleFailure';
import { useLifecycleAgent } from './lifecycleAgent';
import { useLifecycleTransport } from './lifecycleTransport';
import { AgentProvisioningProgress } from './AgentProvisioningProgress';
import { DiscardWaitingCreation } from './DiscardWaitingCreation';
import { agentNameFor, validRepository } from './repository';
import { retainLifecycleHandoff } from '../resources/lifecycleHandoff';
import './Create.css';
import './AgentLifecycleCard.css';

const recoverCreation = { label: 'Recover creation', kind: 'recover' } as const;
const retryStatus = { label: 'Retry status', kind: 'status' } as const;

interface AgentLifecycleCardProps {
  readonly client: CmaWebClient;
  readonly viewerId?: string;
  readonly workspaceId?: string;
  readonly sandboxId?: string;
  readonly canCreateSandbox?: boolean;
  readonly onClose: () => void;
  readonly onCreated: (receipt: AgentLifecycleCreateReceipt) => void;
}

export function AgentLifecycleCard(props: AgentLifecycleCardProps) {
  const { client, viewerId, workspaceId, sandboxId } = props;
  const [scope, setScope] = useState({ client, viewerId, workspaceId, sandboxId, generation: 0 });
  if (
    scope.client !== client ||
    scope.viewerId !== viewerId ||
    scope.workspaceId !== workspaceId ||
    scope.sandboxId !== sandboxId
  )
    setScope({ client, viewerId, workspaceId, sandboxId, generation: scope.generation + 1 });
  return <AgentLifecycleForm key={scope.generation} {...props} />;
}

function AgentLifecycleForm({
  client,
  viewerId,
  workspaceId,
  sandboxId,
  canCreateSandbox = true,
  onClose,
  onCreated,
}: AgentLifecycleCardProps) {
  // The action that opened this dialog fixes its destination. The global action
  // always provisions a new Sandbox; contextual actions keep their exact parent.
  const lifecycle = useLifecycleIntent(client, viewerId, workspaceId, sandboxId);
  const capacity = useSandboxCapacity(client, viewerId);
  const newEnvironment = !workspaceId && !sandboxId;
  const [checkingCapacity, setCheckingCapacity] = useState(false);
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);
  const capacityUnavailable =
    newEnvironment &&
    (!capacity.value || Boolean(capacity.error) || capacity.value.available === 0);
  const { pending, operation, connection: connectionRequest } = lifecycle;
  const repositoryRepair = Boolean(viewerId && connectionRequest?.repository_access);
  const createdReceipt = lifecycle.intent?.receipt;
  const failure = lifecycleFailureMessage(
    lifecycle.failure,
    createdReceipt ? retryStatus : recoverCreation,
  );
  const handoffTransport = useLifecycleTransport(
    client,
    operation,
    createdReceipt?.id,
    viewerId,
    lifecycle.intent?.body.machine_included === true,
  );
  const admitted = useLifecycleAgent(
    client,
    operation,
    createdReceipt?.id,
    viewerId,
    workspaceId,
    sandboxId,
  );
  const openTarget = useMemo(
    () =>
      admitted ??
      (operation?.status === 'succeeded' &&
      operation.id === createdReceipt?.id &&
      operation.agent_id
        ? { id: operation.agent_id, status: 'ready' }
        : undefined),
    [admitted, createdReceipt?.id, operation],
  );
  const [repository, setRepository] = useState('');
  const [machineIncluded, setMachineIncluded] = useState(false);
  // Shown only after the field is left or Create agent is pressed, never while
  // somebody is still typing the URL, so the alert is announced once.
  const [repositoryError, setRepositoryError] = useState(false);
  const openedOperation = useRef<string | undefined>(undefined);
  const clearIntent = lifecycle.clear;
  const bindAgent = lifecycle.bindAgent;
  useEffect(() => {
    if (createdReceipt === undefined || operation === undefined || openTarget === undefined) return;
    if (openedOperation.current === operation.id) return;
    // Handoff as soon as the admitted Agent is real (including `starting`) so
    // the Agent surface can show the server-owned lifecycle stages. A
    // succeeded operation has no separate admission read, so its receipt is
    // opened directly after the server reports the final Agent identity.
    openedOperation.current = operation.id;
    if (operation.status === 'succeeded' ? clearIntent() : bindAgent(openTarget.id)) {
      retainLifecycleHandoff(client.resourceViews, viewerId, admitted);
      handoffTransport();
      onCreated({ ...createdReceipt, id: openTarget.id, status: openTarget.status });
    }
  }, [
    admitted,
    bindAgent,
    clearIntent,
    client.resourceViews,
    createdReceipt,
    handoffTransport,
    onCreated,
    operation,
    openTarget,
    viewerId,
  ]);
  const close = () => {
    lifecycle.abandon();
    onClose();
  };
  const repositoryUrl = repository.trim();
  const repositoryValid = validRepository(repositoryUrl);
  const canCreate = workspaceId !== undefined || sandboxId !== undefined || canCreateSandbox;
  const machineIncludedAvailable =
    workspaceId === undefined && sandboxId === undefined && canCreateSandbox;
  useEffect(() => {
    // Fetch static viewer code during creation; the owned Agent page opens the stream.
    if (machineIncludedAvailable && machineIncluded)
      void import('@novnc/novnc').catch(() => undefined);
  }, [machineIncludedAvailable, machineIncluded]);
  const submit = async () => {
    if (!canCreate) return;
    if (!repositoryValid) {
      setRepositoryError(true);
      document.getElementById('agent-repository')?.focus();
      return;
    }
    const input: AgentLifecycleCreateInput = {
      name: agentNameFor(repositoryUrl),
      ...(machineIncludedAvailable && machineIncluded ? { machine_included: true } : {}),
      ...(workspaceId === undefined ? {} : { workspace_id: workspaceId }),
      ...(sandboxId === undefined ? {} : { sandbox_id: sandboxId }),
      ...(repositoryUrl === '' ? {} : { repository: { url: repositoryUrl } }),
    };
    if (newEnvironment) {
      setCheckingCapacity(true);
      const current = await capacity.ensureFresh();
      if (!mounted.current) return;
      setCheckingCapacity(false);
      if (!current || current.available === 0) return;
    }
    void lifecycle.submit(input);
  };
  const form = (
    <>
      <div className="create-form__field">
        <label className="field__label" htmlFor="agent-repository">
          GitHub repository (optional)
        </label>
        <Input
          id="agent-repository"
          type="url"
          placeholder="https://github.com/org/repository"
          value={repository}
          onChange={(event) => {
            setRepository(event.target.value);
            if (repositoryError && validRepository(event.target.value.trim()))
              setRepositoryError(false);
          }}
          onBlur={() => setRepositoryError(!repositoryValid)}
          aria-invalid={repositoryError || undefined}
          aria-describedby={
            repositoryError
              ? 'agent-repository-help agent-repository-error'
              : 'agent-repository-help'
          }
        />
        <p id="agent-repository-help" className="field__hint">
          Paste a GitHub link if the Agent should work on existing code, or leave this empty to
          start from scratch. If the repository needs authorization, you’ll be asked to connect your
          GitHub account.
        </p>
        {repositoryError ? (
          <p id="agent-repository-error" className="field__error" role="alert">
            Enter an HTTPS GitHub repository URL.
          </p>
        ) : null}
      </div>
      <div className="agent-lifecycle-machine">
        <label className="agent-lifecycle-machine__control" htmlFor="agent-machine-included">
          <input
            id="agent-machine-included"
            type="checkbox"
            checked={machineIncluded}
            disabled={!machineIncludedAvailable}
            onChange={(event) => setMachineIncluded(event.target.checked)}
            aria-describedby="agent-machine-included-help"
          />
          <span>Include a shared desktop</span>
        </label>
        <p id="agent-machine-included-help" className="field__hint">
          {machineIncludedAvailable
            ? 'Adds a Linux desktop with the Chromium browser that you and your Agent can both see and control. Useful for work on websites.'
            : 'A shared desktop can only be added with a new Sandbox; an existing Sandbox keeps its original setup.'}
        </p>
      </div>
      {/*
       * Read-only on purpose: the lifecycle request sends no inline
       * settings, so the summary states the server's defaults rather
       * than offering a choice it would not honour.
       */}
      <section className="agent-lifecycle-settings" aria-labelledby="agent-settings-title">
        <span className="agent-lifecycle-settings__icon" aria-hidden="true">
          <IconAgentPresetOutline16 />
        </span>
        <div>
          <h3 id="agent-settings-title">Agent settings</h3>
          <p>gpt-6 · High reasoning · Autonomous</p>
          <p>The Agent works without asking first. Change this anytime from the message box.</p>
        </div>
      </section>
    </>
  );
  return (
    <>
      <Modal
        open={connectionRequest === undefined || repositoryRepair}
        title="New agent"
        description={
          workspaceId !== undefined
            ? 'Start an Agent in the selected workspace.'
            : sandboxId !== undefined
              ? 'Start an Agent with a new workspace in this Sandbox.'
              : 'Your Agent gets its own Sandbox, a private computer it works in, starting with an empty Workspace.'
        }
        onClose={() => {
          if (operation?.status === 'succeeded' || operation?.status === 'failed')
            lifecycle.clear();
          close();
        }}
        dismissible={!pending}
        footer={
          createdReceipt !== undefined ? (
            <>
              <Button
                onClick={() => {
                  if (operation?.status === 'succeeded' || operation?.status === 'failed')
                    lifecycle.clear();
                  close();
                }}
              >
                Close
              </Button>
              <Button
                variant="primary"
                disabled={openTarget === undefined}
                onClick={() => {
                  if (
                    openTarget &&
                    operation &&
                    openedOperation.current !== operation.id &&
                    (operation.status === 'succeeded'
                      ? lifecycle.clear()
                      : lifecycle.bindAgent(openTarget.id))
                  ) {
                    openedOperation.current = operation.id;
                    retainLifecycleHandoff(client.resourceViews, viewerId, admitted);
                    handoffTransport();
                    onCreated({ ...createdReceipt, id: openTarget.id, status: openTarget.status });
                  }
                }}
              >
                Open agent
              </Button>
            </>
          ) : (
            <>
              <p className="agent-lifecycle-note">
                Includes access to ChronoAI models and the Ornn skill library.
              </p>
              <Button disabled={pending} onClick={close}>
                Cancel
              </Button>
              <Button
                variant="primary"
                disabled={
                  pending ||
                  checkingCapacity ||
                  lifecycle.blocked ||
                  (!lifecycle.intent && (!canCreate || capacityUnavailable))
                }
                onClick={lifecycle.intent ? () => void lifecycle.submit() : submit}
              >
                {pending
                  ? 'Starting provisioning…'
                  : lifecycle.intent
                    ? recoverCreation.label
                    : 'Create agent'}
              </Button>
            </>
          )
        }
      >
        {newEnvironment ? <SandboxCapacityPanel client={client} capacity={capacity} /> : null}
        {createdReceipt !== undefined ? (
          <AgentProvisioningProgress receipt={createdReceipt} operation={operation} />
        ) : null}
        {createdReceipt === undefined ? (
          <div className="agent-lifecycle-step">
            {!canCreate ? (
              <p className="field__hint">You don’t have permission to create a Sandbox.</p>
            ) : null}
            {lifecycle.intent === undefined ? form : null}
          </div>
        ) : null}
        {lifecycle.intent && !createdReceipt ? (
          <p role="status">
            {pending ? (
              <>Starting your Agent…</>
            ) : (
              <>
                Creation of “{lifecycle.intent.body.name}” has not been confirmed. Recover this
                request to continue.
              </>
            )}
          </p>
        ) : null}
        {failure ? (
          <p className="create-error" role="alert">
            {failure}
          </p>
        ) : null}
        {createdReceipt && failure ? (
          <Button onClick={lifecycle.retryStatus}>{retryStatus.label}</Button>
        ) : null}
        {operation?.status === 'waiting_for_connection' && !connectionRequest ? (
          <Button disabled={pending} onClick={() => lifecycle.setConnection(operation.connection)}>
            Connect services
          </Button>
        ) : null}
        {viewerId && connectionRequest?.repository_access ? (
          <RepositoryConnectionRepair
            owner={viewerId}
            {...connectionRequest}
            pending={pending || lifecycle.blocked}
            onRecheck={() => void lifecycle.submit()}
          />
        ) : null}
        {lifecycle.canDiscardWaiting || lifecycle.canCancelWaiting ? (
          <DiscardWaitingCreation
            key={lifecycle.intent?.key}
            label="creation"
            remote={lifecycle.canCancelWaiting}
            onDiscard={async () => {
              if (!(await lifecycle.finishWaiting())) return;
              setRepository('');
              setRepositoryError(false);
              setMachineIncluded(false);
              requestAnimationFrame(() => document.getElementById('agent-repository')?.focus());
            }}
          />
        ) : null}
      </Modal>
      {connectionRequest === undefined || repositoryRepair ? null : (
        <ConnectionWizard
          target={connectionRequest.target}
          expectedOwner={viewerId}
          presentation="dialog"
          onReady={() => {
            lifecycle.setConnection(undefined);
            void lifecycle.submit();
          }}
          onCancel={() => lifecycle.setConnection(undefined)}
        />
      )}
    </>
  );
}
