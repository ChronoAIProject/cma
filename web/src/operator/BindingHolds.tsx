import { useCallback, useEffect, useRef, useState } from 'react';
import { ApiError, type BindingHold, type BindingHoldOverview } from '@cma/client';
import { Button } from '../ui/Button';
import type { OperatorClient } from './client';
import './BindingHolds.css';

type ReadState = {
  readonly client: OperatorClient;
  readonly busy: boolean;
  readonly data?: BindingHoldOverview;
  readonly error?: string;
  readonly denied?: boolean;
  readonly retryAt?: number;
};

export function BindingHolds({ client }: { readonly client: OperatorClient }) {
  const [state, setState] = useState<ReadState>({ client, busy: true });
  const [waiting, setWaiting] = useState(false);
  const active = useRef<AbortController | undefined>(undefined);
  const retryAt = useRef(0);
  const denied = useRef(false);
  const refresh = useCallback(() => {
    if (active.current || denied.current || Date.now() < retryAt.current) return;
    const request = new AbortController();
    active.current = request;
    setState({ client, busy: true });
    void client.admin.bindingHolds({ signal: request.signal, timeoutMs: 10_000 }).then(
      (data) => {
        if (request.signal.aborted || active.current !== request) return;
        active.current = undefined;
        retryAt.current = 0;
        setState({ client, busy: false, data });
      },
      (error: unknown) => {
        if (request.signal.aborted || active.current !== request) return;
        active.current = undefined;
        denied.current = error instanceof ApiError && [401, 403].includes(error.status);
        retryAt.current = Date.now() + (error instanceof ApiError ? (error.retryAfterMs ?? 0) : 0);
        setState({
          client,
          busy: false,
          denied: denied.current,
          retryAt: retryAt.current,
          error: denied.current
            ? 'Current permission could not be confirmed. Check your account access to continue.'
            : 'Runtime binding observations are unavailable. Refresh to check again.',
        });
      },
    );
  }, [client]);

  useEffect(() => {
    retryAt.current = 0;
    denied.current = false;
    refresh();
    return () => {
      active.current?.abort();
      active.current = undefined;
    };
  }, [refresh]);

  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    const update = () => {
      const remaining = (state.retryAt ?? 0) - Date.now();
      setWaiting(remaining > 0);
      if (remaining > 0) timer = setTimeout(update, Math.min(remaining, 60_000));
    };
    update();
    return () => clearTimeout(timer);
  }, [state.retryAt]);

  const current = state.client === client ? state : undefined;
  const data = current?.data;
  return (
    <section className="binding-holds" aria-label="Deferred runtime bindings">
      <header className="binding-holds__header">
        <h2>Deferred runtime bindings</h2>
        <div className="operator-actions">
          <Button
            onClick={refresh}
            disabled={!current || current.busy || current.denied || waiting}
          >
            Refresh binding observations
          </Button>
        </div>
      </header>
      <p className="operator-notice">
        Observations from registered reporters may omit starting or unreachable processes. This view
        does not establish deployment-wide health. Reading it does not retry a binding.
      </p>
      {!current || current.busy ? <p role="status">Reading binding observations…</p> : null}
      {current?.error && (
        <div className="operator-failure" role="alert">
          <p>{current.error}</p>
          {waiting && <p>Wait for the server’s retry delay before refreshing.</p>}
        </div>
      )}
      {data && (
        <>
          <p role="status">
            <strong>{data.observed_sandboxes} observed Sandboxes</strong> · {data.reports.length} of{' '}
            {data.registered_reporters} sampled reporters answered.
          </p>
          <p className="operator-notice">
            Read at <ObservationTime value={data.observed_at} />. Refresh for a new observation.
          </p>
          {(data.unavailable_reporters > 0 || data.truncated) && (
            <p className="binding-holds__incomplete" role="status">
              Incomplete observation. {data.unavailable_reporters} sampled reporters were
              unavailable.
              {data.truncated
                ? ' The result reached a reporting limit or omitted unavailable records.'
                : ''}
            </p>
          )}
          {data.observed_sandboxes === 0 ? (
            <p>No held bindings were observed in the returned samples.</p>
          ) : (
            <ul className="binding-holds__groups">
              {groupHolds(data).map((group) => (
                <li key={group.id}>
                  <h3>Sandbox {group.id}</h3>
                  <ul className="binding-holds__observations">
                    {group.observations.map(({ reporter, observedAt, hold }) => (
                      <li key={reporter}>
                        <h4>Reporter {reporter}</h4>
                        <p className="operator-notice">
                          Captured <ObservationTime value={observedAt} />
                        </p>
                        <dl>
                          <dt>Owner</dt>
                          <dd>{hold.owner_name ?? 'Name unavailable'}</dd>
                          <dt>Hold reason</dt>
                          <dd>{reasons[hold.reason]}</dd>
                          <dt>First held in this process</dt>
                          <dd>
                            <ObservationTime value={hold.first_held_at} />
                          </dd>
                          <dt>Attempts in this process</dt>
                          <dd>{hold.attempts}</dd>
                          <dt>Retry</dt>
                          <dd>
                            {hold.phase === 'retrying' ? (
                              'Retrying at observation time'
                            ) : (
                              <>
                                Eligible no earlier than{' '}
                                <ObservationTime value={hold.retry_not_before!} />
                              </>
                            )}
                          </dd>
                          <dt>Last adapter error class</dt>
                          <dd>
                            {hold.adapter_error_class === 'unavailable'
                              ? 'Unavailable'
                              : 'Not recorded'}
                          </dd>
                        </dl>
                      </li>
                    ))}
                  </ul>
                </li>
              ))}
            </ul>
          )}
        </>
      )}
    </section>
  );
}

const reasons: Record<BindingHold['reason'], string> = {
  unavailable: 'Unavailable',
  raced: 'Concurrent change',
  cancelled: 'Cancelled attempt',
  deadline: 'Deadline reached',
  corrupt: 'Invalid retained state',
  conflict: 'State conflict',
  refused: 'Refused',
};

function ObservationTime({ value }: { readonly value: string }) {
  return <time dateTime={value}>{new Date(value).toLocaleString()}</time>;
}

function groupHolds(data: BindingHoldOverview) {
  const groups = new Map<string, { reporter: string; observedAt: string; hold: BindingHold }[]>();
  for (const { reporter, sample } of data.reports) {
    for (const hold of sample.holds) {
      const entries = groups.get(hold.sandbox_id) ?? [];
      entries.push({ reporter, observedAt: sample.observed_at, hold });
      groups.set(hold.sandbox_id, entries);
    }
  }
  return [...groups].map(([id, observations]) => ({ id, observations }));
}
