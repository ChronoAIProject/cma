import { useEffect, useMemo, useState } from 'react';
import type {
  AgentLifecycleCreateInput,
  AgentLifecycleCreateReceipt,
  CmaWebClient,
} from '../api/client';
import { ConnectionWizard } from '../connections/ConnectionWizard';
import { Button } from '../ui/Button';
import { Input } from '../ui/Input';
import { Modal } from '../ui/Modal';
import { useProfileViewsClient } from '../api/context';
import { ServicePicker } from '../profiles/pickers/ServicePicker';
import { SkillPicker } from '../profiles/pickers/SkillPicker';
import type { SkillReference } from '../profiles/client';
import { useLifecycleIntent } from './lifecycleIntent';
import { AgentProvisioningProgress } from './AgentProvisioningProgress';
import { listAgentImages, type AgentImageChoice } from './imageCatalog';
import './Create.css';
import './AgentLifecycleCard.css';

const SYSTEM_IMAGE: AgentImageChoice = {
  id: 'system-default',
  name: 'System default image',
  description: 'Maintained by Agent Manager and kept compatible with the default runtime.',
};
const EFFORT_CHOICES = ['minimal', 'low', 'medium', 'high', 'x_high'] as const;

type Step = 'image' | 'profile' | 'review';

interface AgentLifecycleCardProps {
  readonly client: CmaWebClient;
  readonly viewerId?: string;
  readonly onClose: () => void;
  readonly onCreated: (receipt: AgentLifecycleCreateReceipt) => void;
}

export function AgentLifecycleCard({
  client,
  viewerId,
  onClose,
  onCreated,
}: AgentLifecycleCardProps) {
  const catalog = useProfileViewsClient()?.authoring;
  const lifecycle = useLifecycleIntent(client, viewerId);
  const { pending, failure, operation, connection: connectionRequest } = lifecycle;
  const createdReceipt = lifecycle.intent?.receipt;
  const [step, setStep] = useState<Step>('image');
  const [images, setImages] = useState<readonly AgentImageChoice[]>([SYSTEM_IMAGE]);
  const [selectedImage, setSelectedImage] = useState(SYSTEM_IMAGE.id);
  const [imagesLoading, setImagesLoading] = useState(true);
  const [imagesError, setImagesError] = useState<string>();
  const [name, setName] = useState('');
  const [repository, setRepository] = useState('');
  const [model, setModel] = useState('');
  const [effort, setEffort] = useState<(typeof EFFORT_CHOICES)[number]>('medium');
  const [permission, setPermission] = useState<'guarded' | 'autonomous'>('guarded');
  const [systemPrompt, setSystemPrompt] = useState('');
  const [scopes, setScopes] = useState<readonly string[]>([]);
  const [skills, setSkills] = useState<readonly SkillReference[]>([]);
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
  const submit = () => {
    const input: AgentLifecycleCreateInput = {
      name: name.trim(),
      ...(selected.id === SYSTEM_IMAGE.id ? {} : { image: { id: selected.id } }),
      ...(repository.trim() === '' ? {} : { repository: { url: repository.trim() } }),
      profile: {
        model: model.trim() || undefined,
        reasoning_effort: effort,
        permission_mode: permission,
        system_prompt: systemPrompt,
        skills: skills.map(({ id, version }) => ({ id, version })),
        permissions: { services: scopes },
      },
    };
    void lifecycle.submit(input);
  };
  const validName =
    name.trim().length > 0 &&
    [...name.trim()].length <= 64 &&
    new TextEncoder().encode(name.trim()).length <= 80 &&
    ![...name].some(
      (value) =>
        value.charCodeAt(0) < 32 || (value.charCodeAt(0) >= 127 && value.charCodeAt(0) <= 159),
    );
  return (
    <>
      <Modal
        open={connectionRequest === undefined}
        title="New agent"
        description="Agent Manager provisions the sandbox, workspace and runtime from this request."
        onClose={() => {
          if (operation?.status === 'succeeded' || operation?.status === 'failed')
            lifecycle.clear();
          onClose();
        }}
        dismissible={!pending}
        placement="modal--wide"
        footer={
          <>
            {createdReceipt !== undefined ? (
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
                    if (
                      operation?.status === 'succeeded' &&
                      operation.agent_id &&
                      lifecycle.clear()
                    )
                      onCreated({ ...createdReceipt, id: operation.agent_id, status: 'ready' });
                  }}
                >
                  Open agent
                </Button>
              </>
            ) : (
              <>
                <Button
                  disabled={pending || lifecycle.intent !== undefined}
                  onClick={() =>
                    step === 'image' ? onClose() : setStep(step === 'review' ? 'profile' : 'image')
                  }
                >
                  Back
                </Button>
                {step === 'review' || lifecycle.intent !== undefined ? (
                  <Button
                    variant="primary"
                    disabled={pending || lifecycle.blocked || (!lifecycle.intent && !validName)}
                    onClick={submit}
                  >
                    {pending
                      ? 'Starting provisioning…'
                      : lifecycle.intent
                        ? 'Recover creation'
                        : 'Create agent'}
                  </Button>
                ) : (
                  <Button
                    variant="primary"
                    disabled={lifecycle.blocked || (step === 'image' ? !validName : false)}
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
          <AgentProvisioningProgress receipt={createdReceipt} operation={operation} />
        ) : null}
        {createdReceipt === undefined && lifecycle.intent === undefined ? (
          <ol className="agent-lifecycle-steps" aria-label="Agent provisioning steps">
            {(['image', 'profile', 'review'] as const).map((item, index) => (
              <li key={item} aria-current={step === item ? 'step' : undefined}>
                <span>{index + 1}</span>{' '}
                {item === 'image' ? 'Image' : item === 'profile' ? 'Agent setup' : 'Review'}
              </li>
            ))}
          </ol>
        ) : null}
        {createdReceipt === undefined && lifecycle.intent === undefined && step === 'image' ? (
          <div className="agent-lifecycle-step">
            <div className="create-form__field">
              <label htmlFor="agent-name">Agent name</label>
              <Input
                id="agent-name"
                value={name}
                onChange={(event) => setName(event.target.value)}
                maxLength={64}
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
        {createdReceipt === undefined && lifecycle.intent === undefined && step === 'profile' ? (
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
                <Input
                  aria-label="Model"
                  value={model}
                  onChange={(event) => setModel(event.target.value)}
                  placeholder="System default"
                  maxLength={128}
                />
                <small>Leave empty to use the system default model.</small>
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
              <ServicePicker
                catalog={catalog}
                value={scopes}
                onChange={setScopes}
                disabled={pending}
              />
            </fieldset>
            <fieldset className="agent-lifecycle-checks">
              <legend>Skills</legend>
              <SkillPicker
                catalog={catalog}
                value={skills}
                onChange={setSkills}
                disabled={pending}
              />
            </fieldset>
          </div>
        ) : null}
        {createdReceipt === undefined && lifecycle.intent === undefined && step === 'review' ? (
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
                  {model || 'System default'} · {effort} · {permission}
                </dd>
              </div>
              <div>
                <dt>Key scope</dt>
                <dd>{scopes.length === 0 ? 'None selected' : scopes.join(', ')}</dd>
              </div>
              <div>
                <dt>Skills</dt>
                <dd>
                  {skills.length === 0
                    ? 'None selected'
                    : skills.map((skill) => skill.id).join(', ')}
                </dd>
              </div>
            </dl>
            <p className="profile-field__help">
              Provisioning has three observable phases: Sandbox ready, workspace and runtime ready,
              then Agent key scope ready.
            </p>
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
