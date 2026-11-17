import { Fragment, useMemo, useState } from 'react';
import type { ChannelBinding, ChannelUsageEvent, EffectiveAccess } from '@cma/client';
import type { ResourceViewsClient } from './client';
import { useAuthorizedRead } from './useAuthorizedRead';
import { Button } from '../ui/Button';
import './ChannelHistory.css';

const reasons: Record<ChannelUsageEvent['reason'], string> = {
  attachment: 'Channel attached',
  detachment: 'Channel detached',
  reset: 'Channel reset',
  rebind: 'Channel binding changed',
  administrative_intervention: 'Administrator intervention',
  accepted_command: 'Channel message accepted',
};
const platformLabels: Record<string, string> = {
  telegram: 'Telegram',
  lark: 'Lark',
  discord: 'Discord',
};
function actorLabel(actor: ChannelUsageEvent['actor']): string {
  switch (actor.kind) {
    case 'human':
      return 'Person';
    case 'api':
      return 'API caller';
    case 'trigger':
      return 'Trigger launch';
    case 'channel':
      return `${platformLabels[actor.platform] ?? 'External channel'} participant`;
  }
}

/** Exact audit references remain available only inside the manager's details. */
function ActorDetails({ actor }: { readonly actor: ChannelUsageEvent['actor'] }) {
  const references: readonly (readonly [string, string])[] =
    actor.kind === 'channel'
      ? [
          ['Platform', actor.platform],
          ['Sender reference', actor.sender_id],
          ['Conversation reference', actor.chat_id],
          ['Bridge reference', actor.bridge_id],
          ['Grant reference', actor.grant_id],
        ]
      : actor.kind === 'trigger'
        ? [
            ['Actor reference', actor.subject],
            ['Launch reference', actor.launch_id],
          ]
        : actor.kind === 'api' && actor.delegation_id !== null
          ? [
              ['Actor reference', actor.subject],
              ['Delegation reference', actor.delegation_id],
            ]
          : [['Actor reference', actor.subject]];
  return (
    <dl>
      {references.map(([label, reference]) => (
        <Fragment key={label}>
          <dt>{label}</dt>
          <dd>{reference}</dd>
        </Fragment>
      ))}
    </dl>
  );
}
function deadline(access: EffectiveAccess, started: number): number {
  const value = Math.min(
    started + 55_000,
    access.revalidate_at == null ? Infinity : Date.parse(access.revalidate_at),
  );
  if (
    !access.capabilities.includes('manage_bridge') ||
    !Number.isFinite(value) ||
    value <= Date.now()
  )
    throw new Error('Current access does not allow channel history.');
  return value;
}
function Bindings({ items }: { readonly items: readonly ChannelBinding[] }) {
  return items.length === 0 ? (
    <p>No active bindings</p>
  ) : (
    <ul>
      {items.map((binding) => (
        <li key={binding.binding_id}>
          <dl>
            <dt>Binding</dt>
            <dd>{binding.binding_id}</dd>
            <dt>Bridge</dt>
            <dd>{binding.bridge_id}</dd>
            <dt>Generation</dt>
            <dd>{binding.generation}</dd>
            {binding.previous_binding_id === null ? null : (
              <>
                <dt>Previous binding</dt>
                <dd>{binding.previous_binding_id}</dd>
              </>
            )}
            <dt>Attached</dt>
            <dd>
              <time dateTime={binding.attached_at}>
                {new Date(binding.attached_at).toLocaleString()}
              </time>
            </dd>
            <dt>First accepted message</dt>
            <dd>
              {binding.first_used_at === null
                ? 'None recorded'
                : new Date(binding.first_used_at).toLocaleString()}
            </dd>
            <dt>Last accepted message</dt>
            <dd>
              {binding.last_used_at === null
                ? 'None recorded'
                : new Date(binding.last_used_at).toLocaleString()}
            </dd>
          </dl>
        </li>
      ))}
    </ul>
  );
}

/** Private history is loaded only while a manager opens it, under the original access deadline. */
export function ChannelHistory({
  client,
  id,
  enabled = true,
}: {
  readonly client: ResourceViewsClient;
  readonly id: string;
  readonly enabled?: boolean;
}) {
  const [cursors, setCursors] = useState<readonly (string | undefined)[]>([undefined]);
  const cursor = cursors.at(-1);
  const read = useMemo(
    () =>
      !enabled
        ? undefined
        : async (signal: AbortSignal) => {
            const started = Date.now();
            const initial = await client.resourceAccess.effective('agent', id, { signal });
            const expires = deadline(initial, started);
            const value = await client.agents.channelUsageHistory(id, {
              cursor,
              limit: 20,
              signal,
            });
            signal.throwIfAborted();
            const current = await client.resourceAccess.effective('agent', id, { signal });
            const finalDeadline = Math.min(expires, deadline(current, started));
            if (current.policy_revision !== initial.policy_revision || finalDeadline <= Date.now())
              throw new Error('Channel access changed. Refresh to check your current access.');
            return { value, revalidateAt: new Date(finalDeadline).toISOString() };
          },
    [client, id, cursor, enabled],
  );
  const current = useAuthorizedRead(read);
  return (
    <section className="channel-history" aria-label="Channel binding history">
      <p>
        Recorded attachments, changes and accepted channel messages. Creation origin stays with the
        Agent.
      </p>
      <Button size="sm" disabled={!enabled} onClick={current.reload}>
        Refresh channel history
      </Button>
      {current.state.status === 'loading' ? (
        <p role="status">Loading channel history</p>
      ) : current.state.status === 'error' ? (
        <p role="alert">{current.state.message}</p>
      ) : (
        <>
          {current.state.value.items.length === 0 ? (
            <p>No channel events on this page.</p>
          ) : (
            <ol>
              {current.state.value.items.map((event) => (
                <li key={event.revision}>
                  <h3>{reasons[event.reason]}</h3>
                  <p>
                    <time dateTime={event.at}>{new Date(event.at).toLocaleString()}</time> ·{' '}
                    {actorLabel(event.actor)}
                  </p>
                  <details>
                    <summary>Binding changes for event {event.revision}</summary>
                    <h4>Actor attribution</h4>
                    <ActorDetails actor={event.actor} />
                    <h4>Before</h4>
                    <Bindings items={event.before} />
                    <h4>After</h4>
                    <Bindings items={event.after} />
                  </details>
                </li>
              ))}
            </ol>
          )}
          <div className="channel-history__pages">
            <Button
              size="sm"
              disabled={cursors.length === 1}
              onClick={() => setCursors((values) => values.slice(0, -1))}
            >
              Previous channel events
            </Button>
            <Button
              size="sm"
              disabled={current.state.value.next === undefined}
              onClick={() => {
                if (current.state.status !== 'ready') return;
                const next = current.state.value.next;
                if (next !== undefined) setCursors((values) => [...values, next]);
              }}
            >
              Next channel events
            </Button>
          </div>
        </>
      )}
    </section>
  );
}
