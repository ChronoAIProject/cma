import { useEffect, useMemo, useRef, useState } from 'react';
import { ChoiceGroup } from '../profiles/ChoiceGroup';
import { WorkerProfilePicker } from './WorkerProfilePicker';
import type { CreateBot, EventWorkerProfileRecord } from '@chronoai/cma-sdk';
import type { AppRoute } from '../routing/routes';
import type { CreationClient, CreationProfile } from '../create/client';
import { ProfilePicker } from '../create/ProfilePicker';
import '../create/Create.css';
import { Button } from '../ui/Button';
import { Input } from '../ui/Input';
import { Picker } from '../ui/Picker';
import { admissionFor, definiteRefusal, problem, type BotClient } from './model';
import { BotPolicy, type PolicyDraft } from './BotPolicy';
import { useBotRead } from './useBotRead';
import { BotEnvironment, environmentReady, type EnvironmentDraft } from './BotEnvironment';

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
  profiles,
  onNavigate,
  intent,
  retain,
  initialName = '',
  initialProfile,
}: {
  readonly client: BotClient;
  readonly profiles: CreationClient;
  readonly onNavigate: (route: AppRoute) => void;
  readonly intent?: CreationIntent;
  readonly retain: (value: CreationIntent | undefined) => boolean | void;
  readonly initialName?: string;
  readonly initialProfile?: CreationProfile;
}) {
  const [step, setStep] = useState(intent === undefined ? 0 : 3);
  const [name, setName] = useState(initialName);
  const [botId, setBotId] = useState('');
  const [profile, setProfile] = useState<CreationProfile | undefined>(initialProfile);
  const [profileMode, setProfileMode] = useState<'profiles' | 'worker'>('profiles');
  const [worker, setWorker] = useState<EventWorkerProfileRecord>();
  const [environment, setEnvironment] = useState<EnvironmentDraft>({ mode: 'default' });
  const [policy, setPolicy] = useState<PolicyDraft>({
    admission: '',
    senders: '',
    autoFinal: true,
    ackSeconds: '2',
  });
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<string>();
  const active = useRef<AbortController | undefined>(undefined);
  const heading = useRef<HTMLHeadingElement>(null);
  const read = useMemo(
    () => (signal: AbortSignal) => client.bots.connections({ signal }),
    [client],
  );
  const choices = useBotRead(read);
  const selected = choices.value?.find((connection) => connection.connection_ref === botId);
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
  const ready = [
    validName && selected !== undefined && choices.fresh,
    profileMode === 'worker'
      ? worker !== undefined
      : profile?.kind === 'agent' && environmentReady(environment),
    validPolicy,
  ];
  useEffect(() => () => active.current?.abort(), []);
  useEffect(() => {
    heading.current?.focus();
  }, [step]);
  const submit = async () => {
    if (busy || (intent === undefined && !ready.every(Boolean))) return;
    const command =
      intent ??
      ((profileMode === 'worker' ? worker === undefined : profile === undefined) ||
      admission === undefined ||
      selected === undefined
        ? undefined
        : {
            key: crypto.randomUUID(),
            profileName:
              profileMode === 'worker' ? (worker?.source.name ?? '') : (profile?.name ?? ''),
            botName: selected.label,
            ...(profileMode === 'profiles' && environment.mode === 'custom'
              ? {
                  sandboxProfileName: environment.sandbox?.name,
                  workspaceProfileName: environment.workspace?.name,
                }
              : {}),
            body: {
              name,
              connection_ref: selected.connection_ref,
              profile:
                profileMode === 'worker' && worker
                  ? worker.source.agent_profile
                  : {
                      id: profile?.revision.profileId ?? '',
                      revision: profile?.revision.revision ?? 0,
                    },
              ...(profileMode === 'worker' && worker
                ? {
                    event_worker_profile: { id: worker.id, revision: worker.revision },
                    sandbox_profile: worker.source.sandbox_profile,
                    workspace_profile: worker.source.workspace_profile,
                  }
                : {}),
              ...(profileMode === 'profiles' &&
              environment.mode === 'custom' &&
              environment.sandbox &&
              environment.workspace
                ? {
                    sandbox_profile: {
                      id: environment.sandbox.revision.profileId,
                      revision: environment.sandbox.revision.revision,
                    },
                    workspace_profile: {
                      id: environment.workspace.revision.profileId,
                      revision: environment.workspace.revision.revision,
                    },
                  }
                : {}),
              admission,
              reply: { auto_final: policy.autoFinal, ack_after_ms: ack },
            },
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
      if (definiteRefusal(error)) retain(undefined);
    } finally {
      if (!request.signal.aborted) setBusy(false);
    }
  };
  const labels = [
    'Choose an Event Gateway connection',
    'Choose the worker configuration',
    'Choose message rules',
    'Review and create',
  ];
  return (
    <section aria-labelledby="bot-create-title">
      <header className="bots__header">
        <h1 id="bot-create-title">New consumer</h1>
        <Button onClick={() => onNavigate({ kind: 'bots' })}>All consumers</Button>
      </header>
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
            <>
              {client.eventWorkerProfiles === undefined ? null : (
                <ChoiceGroup
                  label="Worker configuration"
                  value={profileMode}
                  onChange={setProfileMode}
                  options={[
                    { value: 'profiles', label: 'CMA profiles' },
                    { value: 'worker', label: 'Event Worker Profile' },
                  ]}
                />
              )}
              {profileMode === 'worker' && client.eventWorkerProfiles ? (
                <WorkerProfilePicker
                  client={client.eventWorkerProfiles}
                  value={worker}
                  onChange={setWorker}
                />
              ) : (
                <>
                  <p>Choose a published profile for the Agent’s model, instructions and skills.</p>
                  <ProfilePicker
                    client={profiles}
                    kind="agent"
                    value={profile}
                    onChange={setProfile}
                  />
                  <BotEnvironment client={profiles} value={environment} onChange={setEnvironment} />
                </>
              )}
            </>
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
                  {intent?.body.event_worker_profile || profileMode === 'worker'
                    ? 'Event Worker Profile'
                    : 'Agent profile'}
                </dt>
                <dd>
                  {intent?.profileName ??
                    (profileMode === 'worker' ? worker?.source.name : profile?.name)}{' '}
                  · Version{' '}
                  {intent?.body.event_worker_profile?.revision ??
                    intent?.body.profile.revision ??
                    (profileMode === 'worker' ? worker?.revision : profile?.revision.revision)}
                </dd>
                <dt>Sandbox profile</dt>
                <dd>
                  {intent?.body.sandbox_profile
                    ? `${intent.sandboxProfileName ?? intent.body.sandbox_profile.id} · Version ${intent.body.sandbox_profile.revision}`
                    : intent === undefined && environment.mode === 'custom'
                      ? `${environment.sandbox?.name} · Version ${environment.sandbox?.revision.revision}`
                      : 'Platform default at creation'}
                </dd>
                <dt>Workspace profile</dt>
                <dd>
                  {intent?.body.workspace_profile
                    ? `${intent.workspaceProfileName ?? intent.body.workspace_profile.id} · Version ${intent.body.workspace_profile.revision}`
                    : intent === undefined && environment.mode === 'custom'
                      ? `${environment.workspace?.name} · Version ${environment.workspace?.revision.revision}`
                      : 'Platform default at creation'}
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
                The consumer uses your account and the selected environment profiles. Connect the
                profile’s services and managed model in NyxID before creating it.
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
