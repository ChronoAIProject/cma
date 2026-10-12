import { useEffect, useMemo, useRef, useState } from 'react';
import { ApiError, type Bot } from '@chronoai/cma-sdk';
import type { AppRoute } from '../routing/routes';
import { Button } from '../ui/Button';
import { RiskConfirmation } from '../ui/RiskConfirmation';
import { BotStatus, BotTestResult } from './BotStatus';
import { definiteRefusal, problem, type BotClient } from './model';
import { useBotRead } from './useBotRead';

type Action = 'test' | 'disable' | 'enable' | 'rotateKeys' | 'delete';
export function BotDetail({
  client,
  id,
  onNavigate,
}: {
  readonly client: BotClient;
  readonly id: string;
  readonly onNavigate: (route: AppRoute) => void;
}) {
  const [busy, setBusy] = useState<Action>();
  const [failure, setFailure] = useState<string>();
  const [confirm, setConfirm] = useState<'delete' | 'rotateKeys'>();
  const [uncertain, setUncertain] = useState<{ action: Action; before: Bot }>();
  const [revision, setRevision] = useState(0);
  const [accepted, setAccepted] = useState<Bot>();
  const deletion = useRef(false);
  const request = useRef<AbortController | undefined>(undefined);
  useEffect(() => () => request.current?.abort(), []);
  const read = useMemo(
    () => async (signal: AbortSignal) => {
      // Revision changes cancel older observations after a successful command.
      void revision;
      try {
        return await client.bots.get(id, { signal });
      } catch (error) {
        if (deletion.current && error instanceof ApiError && error.status === 404) return null;
        throw error;
      }
    },
    [client, id, revision],
  );
  const current = useBotRead(read);
  const bot =
    current.value === undefined
      ? current.error === undefined
        ? accepted
        : undefined
      : current.value;
  const settled =
    bot != null && bot.operation === null && ['active', 'disabled', 'failed'].includes(bot.state);
  const confirmed =
    current.fresh &&
    bot != null &&
    uncertain !== undefined &&
    ((uncertain.action === 'disable' && bot.state === 'disabled') ||
      (uncertain.action === 'enable' && bot.state === 'active') ||
      (uncertain.action === 'rotateKeys' &&
        (bot.channel?.version ?? 0) > (uncertain.before.channel?.version ?? 0)) ||
      (uncertain.action === 'test' &&
        bot.last_test !== null &&
        bot.last_test.tested_at !== uncertain.before.last_test?.tested_at));
  const unconfirmed = uncertain !== undefined && !confirmed;
  const available = current.fresh && settled && busy === undefined && !unconfirmed;
  const run = async (action: Action) => {
    if (!available || bot == null) return;
    const active = new AbortController();
    request.current = active;
    setBusy(action);
    setFailure(undefined);
    setUncertain(undefined);
    if (action === 'delete') deletion.current = true;
    try {
      if (action === 'test') {
        const last_test = await client.bots.test(id, { signal: active.signal });
        if (active.signal.aborted) return;
        setAccepted({ ...bot, last_test });
      } else {
        const result = await client.bots[action](id, { signal: active.signal });
        if (active.signal.aborted) return;
        setAccepted(result);
        if (action === 'delete') deletion.current = true;
      }
      setConfirm(undefined);
      setRevision((value) => value + 1);
    } catch (error) {
      if (active.signal.aborted) return;
      setFailure(problem(error));
      if (!definiteRefusal(error)) setUncertain({ action, before: bot });
      else if (action === 'delete') deletion.current = false;
    } finally {
      if (!active.signal.aborted) setBusy(undefined);
    }
  };
  return (
    <section aria-labelledby="bot-detail-title">
      <header className="bots__header">
        <Button onClick={() => onNavigate({ kind: 'bots' })}>All Bots</Button>
        <h1 id="bot-detail-title">{bot?.name ?? 'Bot'}</h1>
        <Button onClick={current.reload}>Refresh status</Button>
      </header>
      <div className="bots__body">
        {current.error === undefined ? null : <p role="alert">{current.error}</p>}
        {bot === undefined && current.error === undefined ? <p role="status">Loading Bot</p> : null}
        {bot !== null ? null : (
          <p role="status">Bot deleted. Its connection, routes and access have been removed.</p>
        )}
        {bot == null ? null : (
          <>
            <BotStatus bot={bot} />
            <dl className="bots__facts">
              <dt>Provider</dt>
              <dd>{bot.provider === 'cma_codex' ? 'CMA' : bot.provider}</dd>
              <dt>Provider level</dt>
              <dd>{bot.channel?.level ?? 'Not reported'}</dd>
              <dt>Sandbox profile</dt>
              <dd>
                {bot.sandbox_profile
                  ? `${bot.sandbox_profile.id} · Version ${bot.sandbox_profile.revision}`
                  : 'Not reported'}
              </dd>
              <dt>Workspace profile</dt>
              <dd>
                {bot.workspace_profile
                  ? `${bot.workspace_profile.id} · Version ${bot.workspace_profile.revision}`
                  : 'Not reported'}
              </dd>
              <dt>Access expiry</dt>
              <dd>
                {bot.credentials_expire_at === null
                  ? 'Not reported'
                  : new Date(bot.credentials_expire_at).toLocaleString()}
              </dd>
            </dl>
            {bot.channel === null ? null : (
              <div>
                <h2>Callback URL</h2>
                <code className="bots__callback">{bot.channel.callback_url}</code>
              </div>
            )}
            <h2>Last test</h2>
            <BotTestResult result={bot.last_test} />
            <div className="bots__actions">
              <Button
                disabled={!available || bot.state !== 'active'}
                onClick={() => void run('test')}
              >
                Run synthetic test
              </Button>
              <Button
                disabled={!available || !['active', 'disabled'].includes(bot.state)}
                onClick={() => void run(bot.state === 'disabled' ? 'enable' : 'disable')}
              >
                {bot.state === 'disabled' ? 'Enable Bot' : 'Disable Bot'}
              </Button>
              <Button
                disabled={!available || !['active', 'disabled'].includes(bot.state)}
                onClick={() => setConfirm('rotateKeys')}
              >
                Rotate keys
              </Button>
              <Button variant="danger" disabled={!available} onClick={() => setConfirm('delete')}>
                Delete Bot
              </Button>
            </div>
            {bot.operation === null ? null : (
              <p>Actions become available when the current operation finishes.</p>
            )}
            {current.fresh ? null : <p>Current status must be checked before another action.</p>}
            {busy === undefined ? null : (
              <p role="status">Waiting for CMA to accept the request.</p>
            )}
            {failure === undefined ? null : <p role="alert">{failure}</p>}
            {!unconfirmed ? null : (
              <p role="status">
                The {uncertain.action === 'rotateKeys' ? 'key rotation' : uncertain.action} outcome
                is unconfirmed. Refresh status to inspect recorded progress. This page will not
                repeat the request automatically.
              </p>
            )}
            {confirm !== 'rotateKeys' ? null : (
              <div>
                <p>
                  Replace this Bot’s access keys. CMA updates its connection before revoking the old
                  keys.
                </p>
                <div className="bots__actions">
                  <Button disabled={busy !== undefined} onClick={() => setConfirm(undefined)}>
                    Cancel rotation
                  </Button>
                  <Button disabled={!available} onClick={() => void run('rotateKeys')}>
                    Confirm key rotation
                  </Button>
                </div>
              </div>
            )}
            <RiskConfirmation
              open={confirm === 'delete'}
              title={`Delete ${bot.name}`}
              description="This removes the Bot’s connection, routes and access. It stops receiving new channel messages. Existing CMA conversations remain retained."
              acknowledgeLabel="I understand this Bot connection will be removed."
              confirmLabel="Confirm deletion"
              busy={busy !== undefined}
              confirmDisabled={!available}
              onCancel={() => setConfirm(undefined)}
              onConfirm={() => void run('delete')}
              failure={failure}
            />
          </>
        )}
      </div>
    </section>
  );
}
