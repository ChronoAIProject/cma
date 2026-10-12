import { useEffect, useMemo, useState } from 'react';
import type {
  AgentLifecycleCreateInput,
  AgentLifecycleCreateReceipt,
  CmaWebClient,
  CommandOptions,
} from '../api/client';
import { AgentLifecycleCreateError } from '../api/client';
import { ConnectionWizard } from '../connections/ConnectionWizard';
import type { ConnectionTarget } from '../connections/client';
import { Button } from '../ui/Button';
import { Input } from '../ui/Input';
import { Modal } from '../ui/Modal';
import { listAgentImages, type AgentImageChoice } from './imageCatalog';
import './Create.css';
import './AgentLifecycleCard.css';

const SYSTEM_IMAGE: AgentImageChoice = {
  id: 'system-default',
  name: 'System default image',
  description: 'Maintained by Agent Manager and kept compatible with the default runtime.',
};
const SCOPE_CHOICES = ['github', 'slack', 'linear', 'storage'] as const;
const SKILL_CHOICES = ['git', 'code-review', 'test-runner'] as const;
const MODEL_CHOICES = ['provider-default', 'gpt-5', 'gpt-5-mini'] as const;
const EFFORT_CHOICES = ['minimal', 'low', 'medium', 'high', 'x_high'] as const;

type Step = 'image' | 'profile' | 'review';

interface AgentLifecycleCardProps {
  readonly client: CmaWebClient;
  readonly onClose: () => void;
  readonly onCreated: (receipt: AgentLifecycleCreateReceipt) => void;
}

export function AgentLifecycleCard({ client, onClose, onCreated }: AgentLifecycleCardProps) {
  const [step, setStep] = useState<Step>('image');
  const [images, setImages] = useState<readonly AgentImageChoice[]>([SYSTEM_IMAGE]);
  const [selectedImage, setSelectedImage] = useState(SYSTEM_IMAGE.id);
  const [imagesLoading, setImagesLoading] = useState(true);
  const [imagesError, setImagesError] = useState<string>();
  const [name, setName] = useState('');
  const [repository, setRepository] = useState('');
  const [model, setModel] = useState<(typeof MODEL_CHOICES)[number]>('provider-default');
  const [effort, setEffort] = useState<(typeof EFFORT_CHOICES)[number]>('medium');
  const [permission, setPermission] = useState<'guarded' | 'autonomous'>('guarded');
  const [systemPrompt, setSystemPrompt] = useState('');
  const [scopes, setScopes] = useState<readonly string[]>([]);
  const [skills, setSkills] = useState<readonly string[]>([]);
  const [pending, setPending] = useState(false);
  const [failure, setFailure] = useState<string>();
  const [connectionRequest, setConnectionRequest] = useState<{
    readonly services: readonly string[];
    readonly target: ConnectionTarget;
  }>();
  const [createdReceipt, setCreatedReceipt] = useState<AgentLifecycleCreateReceipt>();
  const selected = useMemo(
    () => images.find((image) => image.id === selectedImage) ?? SYSTEM_IMAGE,
    [images, selectedImage],
  );
  useEffect(() => {
    const request = new AbortController();
    void listAgentImages(request.signal)
      .then((catalogue) => {
        if (request.signal.aborted) return;
        setImages([SYSTEM_IMAGE, ...catalogue.filter((image) => !image.retired)]);
        setImagesError(undefined);
      })
      .catch((error: unknown) => {
        if (!request.signal.aborted) {
          setImagesError(error instanceof Error ? error.message : 'Image choices are unavailable.');
          setImages([SYSTEM_IMAGE]);
        }
      })
      .finally(() => {
        if (!request.signal.aborted) setImagesLoading(false);
      });
    return () => request.abort();
  }, []);
  const toggle = (values: readonly string[], value: string) =>
    values.includes(value) ? values.filter((item) => item !== value) : [...values, value];
  const submit = () => {
    if (client.createAgentLifecycle === undefined || !name.trim() || pending) return;
    setPending(true);
    setFailure(undefined);
    const input: AgentLifecycleCreateInput = {
      name: name.trim(),
      ...(selected.id === SYSTEM_IMAGE.id ? {} : { image: { id: selected.id } }),
      ...(repository.trim() === '' ? {} : { repository: { url: repository.trim() } }),
      profile: {
        model: model === 'provider-default' ? undefined : model,
        reasoning_effort: effort,
        permission_mode: permission,
        system_prompt: systemPrompt,
        skills: skills.map((id) => ({ id })),
        permissions: { services: scopes },
      },
    };
    void client
      .createAgentLifecycle(input, {
        idempotencyKey: crypto.randomUUID(),
      } satisfies CommandOptions)
      .then(setCreatedReceipt)
      .catch((error: unknown) => {
        const lifecycleError = error instanceof AgentLifecycleCreateError ? error : undefined;
        const message = error instanceof Error ? error.message : 'Agent creation failed.';
        if (lifecycleError?.connection !== undefined)
          setConnectionRequest({
            services: lifecycleError.connection.services,
            target: lifecycleError.connection.target,
          });
        else setFailure(message);
      })
      .finally(() => setPending(false));
  };
  const validName = name.trim().length > 0 && name.trim().length <= 80;
  return (
    <>
      <Modal
        open
        title="New agent"
        description="Agent Manager provisions the sandbox, workspace and runtime from this request."
        onClose={onClose}
        dismissible={!pending}
        placement="modal--wide"
        footer={
          <>
            {createdReceipt !== undefined ? (
              <>
                <Button onClick={onClose}>Close</Button>
                <Button variant="primary" onClick={() => onCreated(createdReceipt)}>
                  Open agent
                </Button>
              </>
            ) : (
              <>
                <Button
                  disabled={pending}
                  onClick={() =>
                    step === 'image' ? onClose() : setStep(step === 'review' ? 'profile' : 'image')
                  }
                >
                  Back
                </Button>
                {step === 'review' ? (
                  <Button variant="primary" disabled={pending || !validName} onClick={submit}>
                    {pending ? 'Starting provisioning…' : 'Create agent'}
                  </Button>
                ) : (
                  <Button
                    variant="primary"
                    disabled={step === 'image' ? imagesLoading : !validName}
                    onClick={() => setStep(step === 'image' ? 'profile' : 'review')}
                  >
                    Continue
                  </Button>
                )}
              </>
            )}
          </>
        }
      >
        {createdReceipt !== undefined ? (
          <AgentProvisioningProgress receipt={createdReceipt} />
        ) : null}
        {createdReceipt === undefined ? (
          <ol className="agent-lifecycle-steps" aria-label="Agent provisioning steps">
            {(['image', 'profile', 'review'] as const).map((item, index) => (
              <li key={item} aria-current={step === item ? 'step' : undefined}>
                <span>{index + 1}</span>{' '}
                {item === 'image' ? 'Image' : item === 'profile' ? 'Agent setup' : 'Review'}
              </li>
            ))}
          </ol>
        ) : null}
        {createdReceipt === undefined && step === 'image' ? (
          <div className="agent-lifecycle-step">
            <div className="create-form__field">
              <label htmlFor="agent-name">Agent name</label>
              <Input
                id="agent-name"
                value={name}
                onChange={(event) => setName(event.target.value)}
                maxLength={80}
                aria-invalid={!validName || undefined}
              />
              <p className="profile-field__help">
                A clear name helps you find this Agent in the navigation tree.
              </p>
            </div>
            <fieldset className="agent-lifecycle-choices">
              <legend>Runtime image</legend>
              <p className="profile-field__help">
                Image Manager provides private and public images. The system default is always
                available.
              </p>
              {imagesLoading ? <p role="status">Loading images…</p> : null}
              {imagesError ? (
                <p role="status">{imagesError} Using the system default image.</p>
              ) : null}
              {images.map((image) => (
                <label key={image.id} className="agent-lifecycle-choice">
                  <input
                    type="radio"
                    name="agent-image"
                    value={image.id}
                    checked={selectedImage === image.id}
                    onChange={() => setSelectedImage(image.id)}
                  />
                  <span>
                    <strong>{image.name}</strong>
                    <small>
                      {image.description ?? 'No description'}
                      {image.id === SYSTEM_IMAGE.id
                        ? ''
                        : ` · ${image.visibility === 'shared' ? 'Shared with you' : 'Private'}`}
                    </small>
                  </span>
                </label>
              ))}
            </fieldset>
          </div>
        ) : null}
        {createdReceipt === undefined && step === 'profile' ? (
          <div className="agent-lifecycle-step">
            <div className="create-form__field">
              <label htmlFor="agent-repository">GitHub repository workspace (optional)</label>
              <Input
                id="agent-repository"
                type="url"
                placeholder="https://github.com/org/repository"
                value={repository}
                onChange={(event) => setRepository(event.target.value)}
              />
              <p className="profile-field__help">
                Agent Manager asks agent-sandbox to create the workspace directly from this
                repository.
              </p>
            </div>
            <div className="agent-lifecycle-grid">
              <label>
                Model
                <select
                  value={model}
                  onChange={(event) => setModel(event.target.value as typeof model)}
                >
                  {MODEL_CHOICES.map((value) => (
                    <option key={value} value={value}>
                      {value === 'provider-default' ? 'System default' : value}
                    </option>
                  ))}
                </select>
              </label>
              <label>
                Reasoning effort
                <select
                  value={effort}
                  onChange={(event) => setEffort(event.target.value as typeof effort)}
                >
                  {EFFORT_CHOICES.map((value) => (
                    <option key={value} value={value}>
                      {value}
                    </option>
                  ))}
                </select>
              </label>
              <label>
                Permission level
                <select
                  value={permission}
                  onChange={(event) => setPermission(event.target.value as typeof permission)}
                >
                  <option value="guarded">Guarded</option>
                  <option value="autonomous">Autonomous</option>
                </select>
              </label>
            </div>
            <label className="create-form__field">
              System prompt
              <textarea
                className="profile-textarea"
                rows={4}
                value={systemPrompt}
                onChange={(event) => setSystemPrompt(event.target.value)}
                maxLength={32768}
                placeholder="Optional instructions that stay with this Agent."
              />
            </label>
            <fieldset className="agent-lifecycle-checks">
              <legend>Agent key scope</legend>
              {SCOPE_CHOICES.map((value) => (
                <label key={value}>
                  <input
                    type="checkbox"
                    checked={scopes.includes(value)}
                    onChange={() => setScopes(toggle(scopes, value))}
                  />
                  {value}
                </label>
              ))}
            </fieldset>
            <fieldset className="agent-lifecycle-checks">
              <legend>Skills</legend>
              {SKILL_CHOICES.map((value) => (
                <label key={value}>
                  <input
                    type="checkbox"
                    checked={skills.includes(value)}
                    onChange={() => setSkills(toggle(skills, value))}
                  />
                  {value}
                </label>
              ))}
            </fieldset>
          </div>
        ) : null}
        {createdReceipt === undefined && step === 'review' ? (
          <div className="agent-lifecycle-step agent-lifecycle-review">
            <h3>Ready to provision</h3>
            <dl>
              <div>
                <dt>Image</dt>
                <dd>{selected.name}</dd>
              </div>
              <div>
                <dt>Workspace</dt>
                <dd>{repository.trim() || 'Empty workspace'}</dd>
              </div>
              <div>
                <dt>Agent setup</dt>
                <dd>
                  {model === 'provider-default' ? 'System default' : model} · {effort} ·{' '}
                  {permission}
                </dd>
              </div>
              <div>
                <dt>Key scope</dt>
                <dd>{scopes.length === 0 ? 'None selected' : scopes.join(', ')}</dd>
              </div>
              <div>
                <dt>Skills</dt>
                <dd>{skills.length === 0 ? 'None selected' : skills.join(', ')}</dd>
              </div>
            </dl>
            <p className="profile-field__help">
              Provisioning has three observable phases: Sandbox ready, workspace and runtime ready,
              then Agent key scope ready.
            </p>
            {failure ? (
              <p className="create-error" role="alert">
                {failure}
              </p>
            ) : null}
          </div>
        ) : null}
      </Modal>
      <Modal
        open={connectionRequest !== undefined}
        title="Connect required services"
        description="The selected Agent key scope includes services that are not connected yet. CMA will verify the connection before retrying."
        onClose={() => setConnectionRequest(undefined)}
      >
        {connectionRequest === undefined ? null : (
          <>
            <ul className="agent-lifecycle-connection-list">
              {connectionRequest.services.map((service) => (
                <li key={service}>{service}</li>
              ))}
            </ul>
            <ConnectionWizard
              target={connectionRequest.target}
              onReady={() => {
                setConnectionRequest(undefined);
                submit();
              }}
              onCancel={() => setConnectionRequest(undefined)}
            />
          </>
        )}
      </Modal>
    </>
  );
}

function AgentProvisioningProgress({ receipt }: { readonly receipt: AgentLifecycleCreateReceipt }) {
  const status = receipt.status ?? 'creating';
  const sandboxReady = [
    'sandbox_ready',
    'configuring',
    'key_scope_ready',
    'ready',
    'active',
  ].includes(status);
  const runtimeReady = ['configuring', 'key_scope_ready', 'ready', 'active'].includes(status);
  const keyScopeReady = ['key_scope_ready', 'ready', 'active'].includes(status);
  const state = (done: boolean, previous: boolean) =>
    done ? 'Ready' : previous ? 'In progress' : 'Waiting';
  return (
    <section className="agent-lifecycle-provisioning" aria-label="Agent provisioning progress">
      <h3>Agent accepted</h3>
      <p>
        Agent Manager is provisioning the Agent Sandbox and will continue asynchronously. You can
        open the Agent now to follow its live status.
      </p>
      <ol>
        <li data-state={state(sandboxReady, true)}>
          <strong>Sandbox ready</strong>
          <span>{state(sandboxReady, true)}</span>
        </li>
        <li data-state={state(runtimeReady, sandboxReady)}>
          <strong>
            Workspace + skills + model + reasoning effort + system prompt + permission level setup
            ready
          </strong>
          <span>{state(runtimeReady, sandboxReady)}</span>
        </li>
        <li data-state={state(keyScopeReady, runtimeReady)}>
          <strong>Agent key scope configuration ready</strong>
          <span>{state(keyScopeReady, runtimeReady)}</span>
        </li>
      </ol>
      <code>{receipt.id}</code>
    </section>
  );
}
