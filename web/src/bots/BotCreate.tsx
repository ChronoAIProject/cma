import { useEffect, useMemo, useRef, useState } from 'react';
import { PageHeader } from '../ui/PageHeader';
import { useDocumentTitle } from '../ui/useDocumentTitle';
import type { CreateBot } from '@cma/client';
import { routePath, type AppRoute } from '../routing/routes';
import type { CreationClient, CreationProfile } from '../create/client';
import '../create/Create.css';
import { Button } from '../ui/Button';
import { Input } from '../ui/Input';
import { Picker } from '../ui/Picker';
import { admissionFor, definiteRefusal, problem, type BotClient } from './model';
import { BotPolicy, type PolicyDraft } from './BotPolicy';
import { useBotRead } from './useBotRead';
import { useProfileViewsClient } from '../api/context';
import { ServicePicker } from '../profiles/pickers/ServicePicker';
import { SkillPicker } from '../profiles/pickers/SkillPicker';
import type { SkillReference } from '../profiles/client';
import { ConnectionWizard } from '../connections/ConnectionWizard';
import { connectionTargetFrom } from './connectionTarget';
import type { ConnectionTarget } from '../connections/client';
const SYSTEM_IMAGE_NAME = 'System default image';
const EFFORT_CHOICES = ['minimal', 'low', 'medium', 'high', 'x_high'] as const;

export interface CreationIntent {
  readonly body: CreateBot;
  readonly key: string;
  readonly profileName: string;
  readonly botName: string;
  readonly sandboxProfileName?: string;
  readonly workspaceProfileName?: string;
}
export function BotCreate({
  client,
  profiles: _profiles,
  onNavigate,
  intent,
  retain,
  initialName = '',
  initialProfile: _initialProfile,
}: {
  readonly client: BotClient;
  readonly profiles: CreationClient;
  readonly onNavigate: (route: AppRoute) => void;
  readonly intent?: CreationIntent;
  readonly retain: (value: CreationIntent | undefined) => boolean | void;
  readonly initialName?: string;
  readonly initialProfile?: CreationProfile;
}) {
  // Keep the creation client in the boundary for historical setup links.
  void _profiles;
  const [step, setStep] = useState(intent === undefined ? 0 : 3);
  const [name, setName] = useState(initialName);
  const [botId, setBotId] = useState('');
  const catalog = useProfileViewsClient()?.authoring;
  const [repository, setRepository] = useState('');
  const initialAgent =
    _initialProfile?.kind === 'agent' ? _initialProfile.revision.content : undefined;
  const [model, setModel] = useState(initialAgent?.model ?? '');
  const [effort, setEffort] = useState<(typeof EFFORT_CHOICES)[number]>(
    (initialAgent?.thinkingEffort as (typeof EFFORT_CHOICES)[number] | undefined) ?? 'medium',
  );
  const [permission, setPermission] = useState<'guarded' | 'autonomous'>(
    initialAgent?.permissionLevel ?? 'guarded',
  );
  const [systemPrompt, setSystemPrompt] = useState(initialAgent?.systemPrompt ?? '');
  const [scopes, setScopes] = useState<readonly string[]>([]);
  const [skills, setSkills] = useState<readonly SkillReference[]>([]);
  const [policy, setPolicy] = useState<PolicyDraft>({
    admission: '',
    senders: '',
    autoFinal: true,
    ackSeconds: '2',
  });
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<string>();
  const [connectionTarget, setConnectionTarget] = useState<ConnectionTarget>();
  const active = useRef<AbortController | undefined>(undefined);
  const heading = useRef<HTMLHeadingElement>(null);
  const read = useMemo(
    () => (signal: AbortSignal) => client.bots.connections({ signal }),
    [client],
  );
  const choices = useBotRead(read);
  const selected = choices.value?.find((connection) => connection.connection_ref === botId);
  const historicalRevision =
    intent?.body.event_worker_profile?.revision ?? intent?.body.profile?.revision;
  const admission = admissionFor(policy.admission, policy.senders);
  const ack = Math.round(Number(policy.ackSeconds) * 1000);
  const validName =
    name.trim().length > 0 && new TextEncoder().encode(name).length <= 80 && !/\p{Cc}/u.test(name);
  const validPolicy =
    admission !== undefined &&
    /^\d+(?:\.\d{1,3})?$/.test(policy.ackSeconds) &&
    Number.isSafeInteger(ack) &&
    ack >= 0 &&
    ack <= 60000;
  const ready = [validName && selected !== undefined && choices.fresh, true, validPolicy];
  useEffect(() => () => active.current?.abort(), []);
  useEffect(() => {
    heading.current?.focus();
  }, [step]);
  const submit = async () => {
    if (busy || (intent === undefined && !ready.every(Boolean))) return;
    const command =
      intent ??
      (admission === undefined || selected === undefined
        ? undefined
        : {
            key: crypto.randomUUID(),
            profileName: SYSTEM_IMAGE_NAME,
            botName: selected.label,
            body: {
              name,
              connection_ref: selected.connection_ref,
              ...(repository.trim() === '' ? {} : { repository: { url: repository.trim() } }),
              agent: {
                model: model.trim() || undefined,
                reasoning_effort: effort,
                permission_mode: permission,
                system_prompt: systemPrompt,
                skills: skills.map(({ id, version }) => ({ id, version })),
                permissions: { services: scopes },
              },
              admission,
              reply: { auto_final: policy.autoFinal, ack_after_ms: ack },
            } as CreateBot,
          });
    if (command === undefined) return;
    if (retain(command) === false) return;
    const request = new AbortController();
    active.current = request;
    setBusy(true);
    setFailure(undefined);
    try {
      const bot = await client.bots.create(command.body, {
        idempotencyKey: command.key,
        signal: request.signal,
      });
      if (request.signal.aborted) return;
      if (retain(undefined) === false) return;
      onNavigate({ kind: 'bot', botId: bot.bot_id });
    } catch (error) {
      if (request.signal.aborted) return;
      setFailure(problem(error));
      setConnectionTarget(connectionTargetFrom(error));
      if (definiteRefusal(error)) retain(undefined);
    } finally {
      if (!request.signal.aborted) setBusy(false);
    }
  };
  useDocumentTitle('New consumer');
  const labels = [
    'Choose an Event Gateway connection',
    'Configure the Agent inline',
    'Choose message rules',
    'Review and create',
  ];
  return (
    <section aria-labelledby="bot-create-title">
      <div className="bots__header">
        <PageHeader
          breadcrumb={[
            { label: 'Event Gateway' },
            {
              label: 'Consumers',
              href: routePath({ kind: 'bots' }),
              onSelect: () => onNavigate({ kind: 'bots' }),
            },
            { label: 'New consumer' },
          ]}
          title="New consumer"
          headingId="bot-create-title"
        />
      </div>
      <div className="bots__body">
        <div className="bots__form">
          <ol className="bots__steps" aria-label="Consumer creation steps">
            {labels.map((label, index) => (
              <li key={label} aria-current={step === index ? 'step' : undefined}>
                {label}
              </li>
            ))}
          </ol>
          <h2 ref={heading} tabIndex={-1}>
            {labels[step]}
          </h2>
          {step !== 0 ? null : (
            <>
              <label className="bots__field">
                Consumer name
                <Input
                  value={name}
                  onChange={(event) => setName(event.target.value)}
                  aria-describedby="bot-name-help"
                />
              </label>
              <p id="bot-name-help">Use a short name on one line.</p>
              <div className="bots__field">
                <span>Event Gateway connection</span>
                <Picker
                  label="Event Gateway connection"
                  value={botId}
                  options={[
                    {
                      value: '',
                      label:
                        choices.value === undefined
                          ? 'Loading Event Gateway connections'
                          : 'Choose an Event Gateway connection',
                    },
                    ...(choices.value ?? []).map((connection) => ({
                      value: connection.connection_ref,
                      label: connection.label,
                    })),
                  ]}
                  onChange={setBotId}
                  disabled={!choices.fresh}
                />
              </div>
              <p>
                <Button onClick={() => onNavigate({ kind: 'event_gateway' })}>
                  Open Event Gateway
                </Button>
                Complete a channel connection there, then refresh this list.
              </p>
              <Button onClick={choices.reload}>Refresh Event Gateway connections</Button>
              {choices.error === undefined ? null : <p role="alert">{choices.error}</p>}
              {choices.value?.length === 0 ? (
                <p>No ready connections. Complete channel setup in Event Gateway first.</p>
              ) : null}
            </>
          )}
          {step !== 1 ? null : (
            <div className="agent-lifecycle-step">
              <label className="bots__field">
                GitHub repository workspace (optional)
                <Input
                  type="url"
                  placeholder="https://github.com/org/repository"
                  value={repository}
                  onChange={(event) => setRepository(event.target.value)}
                />
              </label>
              <div className="agent-lifecycle-grid">
                <label>
                  Model
                  <Input
                    value={model}
                    onChange={(event) => setModel(event.target.value)}
                    placeholder="System default"
                  />
                </label>
                <label>
                  Reasoning effort
                  <Picker
                    value={effort}
                    onChange={(choice) => setEffort(choice as typeof effort)}
                    label="Reasoning effort"
                    options={EFFORT_CHOICES.map((choice) => ({ value: choice, label: choice }))}
                  />
                </label>
                <label>
                  Permission level
                  <Picker
                    value={permission}
                    onChange={(choice) => setPermission(choice as typeof permission)}
                    label="Permission level"
                    options={[
                      { value: 'guarded', label: 'Guarded' },
                      { value: 'autonomous', label: 'Autonomous' },
                    ]}
                  />
                </label>
              </div>
              <label className="bots__field">
                System prompt
                <textarea
                  className="profile-textarea"
                  rows={4}
                  value={systemPrompt}
                  onChange={(event) => setSystemPrompt(event.target.value)}
                  maxLength={32768}
                />
              </label>
              <fieldset className="agent-lifecycle-checks">
                <legend>Agent key scope</legend>
                <ServicePicker
                  catalog={catalog}
                  value={scopes}
                  onChange={setScopes}
                  disabled={busy}
                />
              </fieldset>
              <fieldset className="agent-lifecycle-checks">
                <legend>Skills</legend>
                <SkillPicker
                  catalog={catalog}
                  value={skills}
                  onChange={setSkills}
                  disabled={busy}
                />
              </fieldset>
            </div>
          )}
          {step !== 2 ? null : <BotPolicy value={policy} onChange={setPolicy} />}
          {step !== 3 ? null : (
            <>
              <dl className="bots__facts">
                <dt>Name</dt>
                <dd>{intent?.body.name ?? name}</dd>
                <dt>Event Gateway connection</dt>
                <dd>{intent?.botName ?? selected?.label ?? 'Unavailable'}</dd>
                <dt>
                  {intent?.body.event_worker_profile
                    ? 'Historical Event Worker Profile'
                    : intent?.body.profile
                      ? 'Historical Agent Profile'
                      : 'Agent setup'}
                </dt>
                <dd>
                  {intent?.profileName ?? SYSTEM_IMAGE_NAME}
                  {historicalRevision === undefined ? '' : ` · Version ${historicalRevision}`}
                </dd>
                <dt>{intent?.body.sandbox_profile ? 'Historical Sandbox Profile' : 'Sandbox'}</dt>
                <dd>
                  {intent?.body.sandbox_profile
                    ? `${intent.sandboxProfileName ?? intent.body.sandbox_profile.id} · Version ${intent.body.sandbox_profile.revision}`
                    : intent?.body.profile
                      ? 'Created from historical Agent setup'
                      : 'Created from inline Agent setup'}
                </dd>
                <dt>
                  {intent?.body.workspace_profile ? 'Historical Workspace Profile' : 'Workspace'}
                </dt>
                <dd>
                  {intent?.body.workspace_profile
                    ? `${intent.workspaceProfileName ?? intent.body.workspace_profile.id} · Version ${intent.body.workspace_profile.revision}`
                    : (intent?.body.repository?.url ?? repository.trim()) || 'Empty workspace'}
                </dd>
                <dt>Message access</dt>
                <dd>
                  {intent === undefined
                    ? policy.admission === 'mention'
                      ? 'Private chats and groups with a mention'
                      : policy.admission === 'allowlist'
                        ? 'Selected senders in private chats'
                        : 'Private chats only'
                    : 'Recovering the original message rules'}
                </dd>
                <dt>Automatic final answer</dt>
                <dd>{(intent?.body.reply.auto_final ?? policy.autoFinal) ? 'On' : 'Off'}</dd>
                <dt>Acknowledgement delay</dt>
                <dd>{(intent?.body.reply.ack_after_ms ?? ack) / 1000} seconds</dd>
              </dl>
              <p>
                The consumer uses your account and the Agent setup shown above. Connect any selected
                services in NyxID so the Agent can use them when responding to messages.
              </p>
              {intent === undefined ? null : (
                <p role="status">
                  Creation has not been confirmed. Recover the same request to avoid creating a
                  second consumer.
                </p>
              )}
            </>
          )}
          {failure === undefined ? null : <p role="alert">{failure}</p>}
          {connectionTarget === undefined ? null : (
            <ConnectionWizard
              target={connectionTarget}
              presentation="dialog"
              dialogTitle="Connect services required by this Agent"
              onReady={() => {
                setConnectionTarget(undefined);
                setFailure('Connections verified. Select Create consumer again to continue.');
              }}
              onCancel={() => setConnectionTarget(undefined)}
            />
          )}
          <footer className="bots__actions">
            <Button
              disabled={busy || intent !== undefined || step === 0}
              onClick={() => setStep((current) => current - 1)}
            >
              Back
            </Button>
            {step < 3 ? (
              <Button
                variant="primary"
                disabled={!ready[step]}
                onClick={() => setStep((current) => current + 1)}
              >
                Continue
              </Button>
            ) : (
              <Button
                variant="primary"
                disabled={busy || (intent === undefined && !ready.every(Boolean))}
                onClick={() => void submit()}
              >
                {busy
                  ? 'Creating consumer'
                  : intent === undefined
                    ? 'Create consumer'
                    : 'Recover creation'}
              </Button>
            )}
          </footer>
        </div>
      </div>
    </section>
  );
}
