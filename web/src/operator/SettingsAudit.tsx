import { useEffect, useState } from 'react';
import type { OperatorClient } from './client';
import { operatorFailure } from './failure';
import { Timestamp } from './records';

export function SettingsAudit({
  client,
  revision,
  allowed,
}: {
  readonly client: OperatorClient;
  readonly revision: number;
  readonly allowed: boolean;
}) {
  const [state, setState] = useState<{ actor?: string; at?: string; message?: string }>({});
  useEffect(() => {
    if (!allowed) return;
    const controller = new AbortController();
    client.admin.listActivity({ action: 'configure', limit: 50, signal: controller.signal }).then(
      (page) => {
        if (controller.signal.aborted) return;
        const record = page.items.find(
          (entry) =>
            entry.action === 'configure' &&
            entry.outcome.status === 'succeeded' &&
            entry.target?.kind === 'resource' &&
            ['platform_settings', 'triggers'].includes(entry.target.id) &&
            entry.detail.revision === String(revision),
        );
        setState(
          record
            ? { actor: record.actor_name ?? 'Name unavailable', at: record.at }
            : { message: 'Last change details are not in the recent activity page.' },
        );
      },
      (error: unknown) => {
        if (!controller.signal.aborted)
          setState({
            message: `Last change details could not be read. ${operatorFailure(error).message}`,
          });
      },
    );
    return () => controller.abort();
  }, [allowed, client, revision]);
  return (
    <p className="operator-notice" role="status">
      {!allowed ? (
        'Last change details require activity-log permission.'
      ) : state.at ? (
        <>
          Last platform settings change: {state.actor}, <Timestamp value={state.at} /> (SGT). This
          audit describes the settings revision, not individual fields.
        </>
      ) : (
        (state.message ?? 'Loading last platform settings change…')
      )}
    </p>
  );
}
