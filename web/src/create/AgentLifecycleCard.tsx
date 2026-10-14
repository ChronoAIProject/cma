import { useEffect, useMemo, useRef, useState } from 'react';
import type {
  AgentLifecycleCreateInput,
  AgentLifecycleCreateReceipt,
  CmaWebClient,
} from '../api/client';
import { ConnectionWizard } from '../connections/ConnectionWizard';
import { Button } from '../ui/Button';
import { Input } from '../ui/Input';
import { Modal } from '../ui/Modal';
import { useLifecycleIntent } from './lifecycleIntent';
import { useLifecycleAgent } from './lifecycleAgent';
import { AgentProvisioningProgress } from './AgentProvisioningProgress';
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

function validRepository(value: string): boolean {
  if (value === '') return true;
  try {
    const url = new URL(value);
    return (
      url.protocol === 'https:' &&
      url.hostname === 'github.com' &&
      url.username === '' &&
      url.password === '' &&
      url.port === '' &&
      url.search === '' &&
      url.hash === '' &&
      /^\/[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+\/?$/.test(url.pathname) &&
      url.pathname
        .split('/')
        .filter(Boolean)
        .every((part) => !['.', '..', '.git'].includes(part))
    );
  } catch {
    return false;
  }
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
  const lifecycle = useLifecycleIntent(client, viewerId, workspaceId, sandboxId);
  const { pending, failure, operation, connection: connectionRequest } = lifecycle;
  const createdReceipt = lifecycle.intent?.receipt;
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
  const [workspaceName, setWorkspaceName] = useState<string>();
  const openedOperation = useRef<string | undefined>(undefined);
  const clearIntent = lifecycle.clear;
  const bindAgent = lifecycle.bindAgent;
  useEffect(() => {
    if (createdReceipt === undefined || operation === undefined || openTarget === undefined) return;
    if (openedOperation.current === operation.id) return;
    openedOperation.current = operation.id;
    if (operation.status === 'succeeded' ? clearIntent() : bindAgent(openTarget.id))
      onCreated({ ...createdReceipt, id: openTarget.id, status: openTarget.status });
  }, [bindAgent, clearIntent, createdReceipt, onCreated, operation, openTarget]);
  useEffect(() => {
    if (workspaceId === undefined) return;
    const request = new AbortController();
    void client
      .readWorkspace(workspaceId, { signal: request.signal })
      .then((workspace) => {
        if (!request.signal.aborted && workspace.id === workspaceId)
          setWorkspaceName(workspace.name);
      })
      .catch(() => undefined);
    return () => request.abort();
  }, [client, workspaceId]);
  const repositoryUrl = repository.trim();
  const repositoryValid = validRepository(repositoryUrl);
  const canCreate = workspaceId !== undefined || sandboxId !== undefined || canCreateSandbox;
  const valid = repositoryValid && canCreate;
  const submit = () => {
    if (!valid) return;
    const input: AgentLifecycleCreateInput = {
      name:
        repositoryUrl === ''
          ? 'New agent'
          : new URL(repositoryUrl).pathname
              .split('/')
              .filter(Boolean)
              .at(-1)!
              .replace(/\.git$/, '')
              .slice(0, 64),
      ...(workspaceId === undefined ? {} : { workspace_id: workspaceId }),
      ...(sandboxId === undefined ? {} : { sandbox_id: sandboxId }),
      ...(repositoryUrl === '' ? {} : { repository: { url: repositoryUrl } }),
    };
    void lifecycle.submit(input);
  };
  return (
    <>
      <Modal
        open={connectionRequest === undefined}
        title="New agent"
        description={
          workspaceId === undefined && sandboxId === undefined
            ? 'Start an Agent in a new Sandbox with an empty workspace.'
            : workspaceId !== undefined
              ? 'Start an Agent in the selected workspace.'
              : 'Start an Agent with a new workspace in this Sandbox.'
        }
        onClose={() => {
          if (operation?.status === 'succeeded' || operation?.status === 'failed')
            lifecycle.clear();
          onClose();
        }}
        dismissible={!pending}
        placement="modal--wide"
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
                    onCreated({ ...createdReceipt, id: openTarget.id, status: openTarget.status });
                  }
                }}
              >
                Open agent
              </Button>
            </>
          ) : (
            <>
              <Button disabled={pending} onClick={onClose}>
                Cancel
              </Button>
              <Button
                variant="primary"
                disabled={pending || lifecycle.blocked || (!lifecycle.intent && !valid)}
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
        {createdReceipt === undefined && lifecycle.intent === undefined ? (
          <div className="agent-lifecycle-step">
            {workspaceId === undefined ? null : (
              <p role="status">Existing Sandbox · {workspaceName ?? 'Selected workspace'}</p>
            )}
            {canCreate ? null : (
              <p role="status">
                You don’t have permission to create a Sandbox. Use New agent from an existing
                Sandbox or workspace.
              </p>
            )}
            <div className="create-form__field">
              <label htmlFor="agent-repository">GitHub repository (optional)</label>
              <Input
                id="agent-repository"
                type="url"
                placeholder="https://github.com/org/repository"
                value={repository}
                onChange={(event) => setRepository(event.target.value)}
                aria-invalid={!repositoryValid || undefined}
                aria-describedby="agent-repository-help"
              />
              <p id="agent-repository-help" className="profile-field__help">
                Your Agent receives this repository in its first message and prepares it in the
                empty workspace. If authorization is needed, you’ll be prompted to connect your
                account.
              </p>
              {repositoryValid ? null : (
                <p className="create-error" role="alert">
                  Enter an HTTPS GitHub repository URL.
                </p>
              )}
            </div>
          </div>
        ) : null}
        {lifecycle.intent && !createdReceipt ? (
          <p role="status">
            Creation of “{lifecycle.intent.body.name}” has not been confirmed. Recover this request
            to continue.
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
