import { useEffect, useMemo, useState } from 'react';
import { ApiError, type AgentBrowserClient, type BrowserObservation } from '@chronoai/cma-sdk';
import { Button } from '../../ui/Button';

type BrowserStatus = BrowserObservation['state'] | 'forbidden';
type Frame = NonNullable<BrowserObservation['frame']>;
const hidden = () => document.visibilityState === 'hidden';

export function BrowserPanel({
  client,
  agentId,
  incarnation,
  active,
  onReload,
}: {
  readonly client: AgentBrowserClient;
  readonly agentId: string;
  readonly incarnation: string;
  readonly active: boolean;
  readonly onReload: () => void;
}) {
  const scope = useMemo(() => ({ client, agentId, incarnation }), [client, agentId, incarnation]);
  const [stored, setStored] = useState<{
    scope: typeof scope;
    state: BrowserStatus;
    frame?: Frame;
    location?: string;
  }>();
  const [decoded, setDecoded] = useState<Frame>();
  const [clock, setClock] = useState(Date.now);
  const current = stored?.scope === scope && active ? stored : undefined;
  useEffect(() => {
    if (!active) return;
    let alive = true;
    let generation = 0;
    let pending: AbortController | undefined;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let contextId: string | undefined;
    let frame: Frame | undefined;
    let location: string | undefined;
    let failures = 0;
    let terminal = false;
    const observer = crypto.randomUUID();
    const publish = (state: BrowserStatus, retained = frame) => {
      if (alive) setStored({ scope, state, frame: retained, location });
    };
    const release = () => {
      void client.release(agentId, { runtime_incarnation: incarnation, observer }).catch(() => {});
    };
    const stop = () => {
      generation += 1;
      clearTimeout(timer);
      pending?.abort();
      pending = undefined;
      release();
    };
    const read = async () => {
      if (!alive || terminal || hidden()) return;
      const version = generation;
      const request = new AbortController();
      pending = request;
      const deadline = setTimeout(() => request.abort(), 4000);
      try {
        const value = await client.read(
          agentId,
          { runtime_incarnation: incarnation, observer, context_id: contextId },
          { signal: request.signal },
        );
        if (!alive || version !== generation || request.signal.aborted) return;
        if (
          contextId !== undefined &&
          value.context_id !== contextId &&
          !['paused', 'not_enabled', 'unsupported'].includes(value.state)
        ) {
          frame = undefined;
          terminal = true;
          publish('disconnected');
          return;
        }
        contextId = value.context_id ?? contextId;
        location = value.location;
        const incoming = value.frame;
        if (
          incoming &&
          Date.parse(incoming.captured_at) <= Date.now() + 1000 &&
          (!frame ||
            (incoming.sequence > frame.sequence &&
              Date.parse(incoming.captured_at) >= Date.parse(frame.captured_at)))
        )
          frame = incoming;
        if (['unsupported', 'not_enabled', 'paused'].includes(value.state)) frame = undefined;
        terminal = ['unsupported', 'not_enabled'].includes(value.state);
        failures = 0;
        publish(value.state);
      } catch (error) {
        if (!alive || version !== generation) return;
        failures += 1;
        if (error instanceof ApiError && [401, 403, 404, 409].includes(error.status)) {
          frame = undefined;
          terminal = true;
          publish(error.status === 409 ? 'disconnected' : 'forbidden');
        } else publish('disconnected');
      } finally {
        clearTimeout(deadline);
        if (pending === request) pending = undefined;
        if (alive && version === generation && !terminal && !hidden())
          timer = setTimeout(
            () => {
              void read();
            },
            Math.min(8000, 1000 * 2 ** failures),
          );
      }
    };
    const visibility = () => {
      stop();
      if (document.visibilityState === 'hidden') publish('paused', undefined);
      else if (!terminal) {
        publish('loading');
        void read();
      }
    };
    document.addEventListener('visibilitychange', visibility);
    publish('loading');
    void read();
    const freshness = setInterval(() => setClock(Date.now()), 250);
    return () => {
      alive = false;
      stop();
      clearInterval(freshness);
      document.removeEventListener('visibilitychange', visibility);
    };
  }, [scope, client, agentId, incarnation, active]);
  const frame = current?.frame;
  const fresh =
    frame &&
    decoded === frame &&
    Date.parse(frame.captured_at) <= clock + 1000 &&
    clock - Date.parse(frame.captured_at) < 3000;
  const live = current?.state === 'live' && fresh;
  const message = live
    ? 'Live'
    : frame
      ? 'Last frame · Reconnecting…'
      : {
          loading: 'Loading Agent browser…',
          ready: 'Waiting for a browser frame…',
          live: 'Loading Agent browser…',
          paused: 'Browser observation paused.',
          disconnected: 'Browser disconnected. Reconnecting…',
          forbidden: 'Browser access is not permitted.',
          unsupported: 'Browser is not supported by this environment.',
          not_enabled: 'Browser is not enabled for this environment.',
        }[current?.state ?? 'loading'];
  return (
    <section className="agent-browser" aria-label="Browser observation">
      <p role="status">{message}</p>
      {frame ? (
        <figure>
          <img
            key={frame.sequence}
            src={`data:image/jpeg;base64,${frame.data}`}
            alt="Agent browser page"
            width={1280}
            height={720}
            onLoad={() => setDecoded(frame)}
            onError={() => setDecoded(undefined)}
          />
          <figcaption>
            {current?.location} ·{' '}
            <time dateTime={frame.captured_at}>
              {new Date(frame.captured_at).toLocaleTimeString()}
            </time>
          </figcaption>
        </figure>
      ) : null}
      <p>Observation only</p>
      {['disconnected', 'forbidden'].includes(current?.state ?? '') ? (
        <Button size="sm" onClick={onReload}>
          Check browser access again
        </Button>
      ) : null}
    </section>
  );
}

export function EnableBrowser({
  client,
  agentId,
  incarnation,
  onEnabled,
}: {
  readonly client: AgentBrowserClient;
  readonly agentId: string;
  readonly incarnation: string;
  readonly onEnabled: () => void;
}) {
  const [pending, setPending] = useState(false);
  const [error, setError] = useState(false);
  useEffect(() => {
    setError(false);
    setPending(false);
  }, [client, agentId, incarnation]);
  const enable = async () => {
    setPending(true);
    setError(false);
    try {
      await client.configure(agentId, incarnation, true);
      onEnabled();
    } catch {
      setError(true);
    } finally {
      setPending(false);
    }
  };
  return (
    <>
      <Button
        size="sm"
        disabled={pending}
        onClick={() => {
          void enable();
        }}
      >
        {pending ? 'Starting browser…' : 'Enable browser'}
      </Button>
      {error ? <p role="alert">Browser could not be enabled. Try again.</p> : null}
    </>
  );
}
