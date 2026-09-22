import { useEffect, useMemo, useRef, useState } from 'react';
import type { CreateBot } from '@chronoai/cma-sdk';
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

export interface CreationIntent {
  readonly body: CreateBot;
  readonly key: string;
  readonly profileName: string;
  readonly botName: string;
}
export function BotCreate({
  client,
  profiles,
  onNavigate,
  intent,
  retain,
}: {
  readonly client: BotClient;
  readonly profiles: CreationClient;
  readonly onNavigate: (route: AppRoute) => void;
  readonly intent?: CreationIntent;
  readonly retain: (value: CreationIntent | undefined) => boolean | void;
}) {
  const [step, setStep] = useState(intent === undefined ? 0 : 3);
  const [name, setName] = useState('');
  const [botId, setBotId] = useState('');
  const [profile, setProfile] = useState<CreationProfile>();
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
  const read = useMemo(() => (signal: AbortSignal) => client.bots.available({ signal }), [client]);
  const choices = useBotRead(read);
  const selected = choices.value?.find((bot) => bot.id === botId && bot.platform === 'telegram');
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
    profile?.kind === 'agent',
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
      (profile === undefined || admission === undefined || selected === undefined
        ? undefined
        : {
            key: crypto.randomUUID(),
            profileName: profile.name,
            botName: selected.name,
            body: {
              name,
              nyxid_bot_id: selected.id,
              profile: { id: profile.revision.profileId, revision: profile.revision.revision },
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
    'Choose a Telegram bot',
    'Choose an Agent profile',
    'Choose message rules',
    'Review and create',
  ];
  return (
    <section aria-labelledby="bot-create-title">
      <header className="bots__header">
        <h1 id="bot-create-title">New Bot</h1>
        <Button onClick={() => onNavigate({ kind: 'bots' })}>All Bots</Button>
      </header>
      <div className="bots__body">
        <div className="bots__form">
          <ol className="bots__steps" aria-label="Bot creation steps">
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
                Bot name
                <Input
                  value={name}
                  onChange={(event) => setName(event.target.value)}
                  aria-describedby="bot-name-help"
                />
              </label>
              <p id="bot-name-help">Use a short name on one line.</p>
              <div className="bots__field">
                <span>Telegram bot</span>
                <Picker
                  label="Telegram bot"
                  value={botId}
                  options={[
                    {
                      value: '',
                      label:
                        choices.value === undefined
                          ? 'Loading Telegram bots'
                          : 'Choose a Telegram bot',
                    },
                    ...(choices.value ?? [])
                      .filter((bot) => bot.platform === 'telegram')
                      .map((bot) => ({ value: bot.id, label: bot.name })),
                  ]}
                  onChange={setBotId}
                  disabled={!choices.fresh}
                />
              </div>
              <p>
                <a
                  href="https://nyx.chrono-ai.fun/channel-bots"
                  target="_blank"
                  rel="noopener noreferrer"
                >
                  Register a Telegram bot in NyxID
                </a>
                , then refresh this list.
              </p>
              <Button onClick={choices.reload}>Refresh Telegram bots</Button>
              {choices.error === undefined ? null : <p role="alert">{choices.error}</p>}
              {choices.value?.length === 0 ? (
                <p>
                  No available Telegram bots. A bot with an existing route cannot be connected here.
                </p>
              ) : null}
            </>
          )}
          {step !== 1 ? null : (
            <>
              <p>Choose a published profile for the Agent’s model, instructions and skills.</p>
              <ProfilePicker client={profiles} kind="agent" value={profile} onChange={setProfile} />
            </>
          )}
          {step !== 2 ? null : <BotPolicy value={policy} onChange={setPolicy} />}
          {step !== 3 ? null : (
            <>
              <dl className="bots__facts">
                <dt>Name</dt>
                <dd>{intent?.body.name ?? name}</dd>
                <dt>Telegram bot</dt>
                <dd>{intent?.botName ?? selected?.name ?? 'Unavailable'}</dd>
                <dt>Agent profile</dt>
                <dd>
                  {intent?.profileName ?? profile?.name} · Version{' '}
                  {intent?.body.profile.revision ?? profile?.revision.revision}
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
                The Bot uses your account and the current default Sandbox and Workspace profiles.
                Connect the profile’s services and CMAEG in NyxID before creating it.
              </p>
              {intent === undefined ? null : (
                <p role="status">
                  Creation has not been confirmed. Recover the same request to avoid creating a
                  second Bot.
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
                {busy ? 'Creating Bot' : intent === undefined ? 'Create Bot' : 'Recover creation'}
              </Button>
            )}
          </footer>
        </div>
      </div>
    </section>
  );
}
