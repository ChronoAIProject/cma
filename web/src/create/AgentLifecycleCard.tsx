import { useEffect, useState } from 'react';
import type { SandboxSummary } from '../api/types';
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
import { AgentProvisioningProgress } from './AgentProvisioningProgress';
import './Create.css';
import './AgentLifecycleCard.css';

interface AgentLifecycleCardProps {
  readonly client: CmaWebClient;
  readonly viewerId?: string;
  readonly workspaceId?: string;
  readonly sandboxId?: string;
  readonly canCreateSandbox?: boolean;
  readonly targetPicker?: React.ReactNode;
  readonly onClose: () => void;
  readonly onCreated: (receipt: AgentLifecycleCreateReceipt) => void;
}

export function AgentLifecycleCard(props: AgentLifecycleCardProps) {
  const contextual = props.workspaceId !== undefined || props.sandboxId !== undefined;
  const [target, setTarget] = useState(props.canCreateSandbox === false ? '' : 'new');
  const [sandboxes, setSandboxes] = useState<readonly SandboxSummary[]>([]);
  const [locationsError, setLocationsError] = useState<string>();
  useEffect(() => {
    if (contextual) return;
    const request = new AbortController();
    void (async () => {
      const items: SandboxSummary[] = [];
      const seen = new Set<string>();
      let cursor: string | undefined;
      do {
        const page = await props.client.listSandboxes({ cursor, signal: request.signal });
        items.push(...page.items);
        cursor = page.next;
        if (cursor !== undefined && seen.has(cursor)) throw new Error();
        if (cursor !== undefined) seen.add(cursor);
      } while (cursor !== undefined && !request.signal.aborted);
      if (!request.signal.aborted)
        setSandboxes(items.filter((sandbox) => sandbox.status === 'active'));
    })().catch(() => {
      if (!request.signal.aborted) setLocationsError('Existing Sandboxes could not be loaded.');
    });
    return () => request.abort();
  }, [contextual, props.client]);
  if (contextual) return <AgentLifecycleForm {...props} />;
  const picker = (
    <div className="agent-lifecycle-location">
      <label htmlFor="agent-sandbox">Agent location</label>
      <select id="agent-sandbox" value={target} onChange={(event) => setTarget(event.target.value)}>
        {props.canCreateSandbox === false ? (
          <option value="">Choose an existing Sandbox</option>
        ) : (
          <option value="new">New Sandbox + new Agent</option>
        )}
        {sandboxes.map((sandbox) => (
          <option key={sandbox.id} value={sandbox.id}>
            Existing Sandbox: {sandbox.name}
          </option>
        ))}
      </select>
      {locationsError ? <p role="status">{locationsError}</p> : null}
    </div>
  );
  if (target === '')
    return (
      <Modal
        open
        title="New agent"
        onClose={props.onClose}
        footer={<Button onClick={props.onClose}>Cancel</Button>}
      >
        {picker}
      </Modal>
    );
  return (
    <AgentLifecycleForm
      key={target}
      {...props}
      sandboxId={target === 'new' ? undefined : target}
      targetPicker={picker}
    />
  );
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

function AgentLifecycleForm({
  client,
  viewerId,
  workspaceId,
  sandboxId,
  targetPicker,
  onClose,
  onCreated,
}: AgentLifecycleCardProps) {
  const lifecycle = useLifecycleIntent(client, viewerId, workspaceId, sandboxId);
  const { pending, failure, operation, connection: connectionRequest } = lifecycle;
  const createdReceipt = lifecycle.intent?.receipt;
  const [repository, setRepository] = useState('');
  const [workspaceName, setWorkspaceName] = useState<string>();
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
  const valid = repositoryValid;
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
            ? 'Start an Agent in a new Sandbox with the CMA runtime and an empty workspace.'
            : 'Start an Agent in this Sandbox with the CMA runtime.'
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
                disabled={operation?.status !== 'succeeded' || operation.agent_id === undefined}
                onClick={() => {
                  if (operation?.status === 'succeeded' && operation.agent_id && lifecycle.clear())
                    onCreated({ ...createdReceipt, id: operation.agent_id, status: 'ready' });
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
            {targetPicker}
            {workspaceId === undefined ? null : (
              <p role="status">Existing Sandbox · {workspaceName ?? 'Selected workspace'}</p>
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
