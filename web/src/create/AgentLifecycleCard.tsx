import { useEffect, useMemo, useRef, useState } from 'react';
import type {
  AgentLifecycleCreateInput,
  AgentLifecycleCreateReceipt,
  CmaWebClient,
} from '../api/client';
import { ConnectionWizard } from '../connections/ConnectionWizard';
import { Button } from '../ui/Button';
import { Input } from '../ui/Input';
import { IconAgentPresetOutline16 } from '../ui/icons';
import { Modal } from '../ui/Modal';
import { useLifecycleIntent } from './lifecycleIntent';
import { useLifecycleAgent } from './lifecycleAgent';
import { useLifecycleTransport } from './lifecycleTransport';
import { AgentProvisioningProgress } from './AgentProvisioningProgress';
import { agentNameFor, validRepository } from './repository';
import './Create.css';
import './AgentLifecycleCard.css';

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
  const { pending, failure, operation, connection: connectionRequest } = lifecycle;
  const createdReceipt = lifecycle.intent?.receipt;
  const handoffTransport = useLifecycleTransport(client, operation, createdReceipt?.id, viewerId);
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
      handoffTransport();
      onCreated({ ...createdReceipt, id: openTarget.id, status: openTarget.status });
    }
  }, [bindAgent, clearIntent, createdReceipt, handoffTransport, onCreated, operation, openTarget]);
  const repositoryUrl = repository.trim();
  const repositoryValid = validRepository(repositoryUrl);
  const canCreate = workspaceId !== undefined || sandboxId !== undefined || canCreateSandbox;
  const submit = () => {
    if (!canCreate) return;
    if (!repositoryValid) {
      setRepositoryError(true);
      document.getElementById('agent-repository')?.focus();
      return;
    }
    const input: AgentLifecycleCreateInput = {
      name: agentNameFor(repositoryUrl),
      ...(workspaceId === undefined ? {} : { workspace_id: workspaceId }),
      ...(sandboxId === undefined ? {} : { sandbox_id: sandboxId }),
      ...(repositoryUrl === '' ? {} : { repository: { url: repositoryUrl } }),
    };
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
          Your Agent receives this repository in its first message and prepares it in the empty
          workspace. If authorization is needed, you’ll be prompted to connect your account.
        </p>
        {repositoryError ? (
          <p id="agent-repository-error" className="field__error" role="alert">
            Enter an HTTPS GitHub repository URL.
          </p>
        ) : null}
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
          <p>gpt-6 · High · Autonomous</p>
        </div>
      </section>
    </>
  );
  return (
    <>
      <Modal
        open={connectionRequest === undefined}
        title="New agent"
        description={
          workspaceId !== undefined
            ? 'Start an Agent in the selected workspace.'
            : sandboxId !== undefined
              ? 'Start an Agent with a new workspace in this Sandbox.'
              : 'Start an Agent in a new Sandbox with an empty workspace.'
        }
        onClose={() => {
          if (operation?.status === 'succeeded' || operation?.status === 'failed')
            lifecycle.clear();
          onClose();
        }}
        dismissible={!pending}
        footer={
          createdReceipt !== undefined ? (
            <>
              <Button
                onClick={() => {
                  if (operation?.status === 'succeeded' || operation?.status === 'failed')
                    lifecycle.clear();
                  onClose();
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
                Includes chrono-llm-public and ornn-api access.
              </p>
              <Button disabled={pending} onClick={onClose}>
                Cancel
              </Button>
              <Button
                variant="primary"
                disabled={pending || lifecycle.blocked || (!lifecycle.intent && !canCreate)}
                onClick={lifecycle.intent ? () => void lifecycle.submit() : submit}
              >
                {pending
                  ? 'Starting provisioning…'
                  : lifecycle.intent
                    ? 'Recover creation'
                    : 'Create agent'}
              </Button>
            </>
          )
        }
      >
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
          <Button onClick={lifecycle.retryStatus}>Retry status</Button>
        ) : null}
        {operation?.status === 'waiting_for_connection' && !connectionRequest ? (
          <Button onClick={() => lifecycle.setConnection(operation.connection)}>
            Connect services
          </Button>
        ) : null}
      </Modal>
      {connectionRequest === undefined ? null : (
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
