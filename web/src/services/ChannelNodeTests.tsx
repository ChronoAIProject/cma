import { useRef, useState } from 'react';
import { Alert } from '../ui/Alert';
import { Button } from '../ui/Button';
import { Presence } from '../ui/Presence';
import { segment, ServiceError, serviceRequest, type Denied } from './client';
import { selectionKey, type ChannelSelection } from './channelTopology';

type Result = {
  text: string;
  uncertain?: boolean;
  checks?: readonly Check[];
  requestedVersion?: number;
};
interface Check {
  id: string;
  label: string;
  status: 'passed' | 'unavailable';
  detail: string;
}
export function ChannelNodeTests({
  channelId,
  channelVersion,
  selection,
  denied,
}: {
  readonly channelId: string;
  readonly channelVersion?: number;
  readonly selection: ChannelSelection;
  readonly denied: Denied;
}) {
  const key = selectionKey(selection);
  const [results, setResults] = useState<Record<string, Result>>({});
  const [pending, setPending] = useState<ReadonlySet<string>>(new Set());
  const running = useRef(new Set<string>());
  const source = selection.kind === 'producer';
  const result = results[key];
  const busy = pending.has(key);
  async function test() {
    if (running.current.has(key)) return;
    running.current.add(key);
    setPending(new Set(running.current));
    setResults((values) => ({ ...values, [key]: { text: '', requestedVersion: channelVersion } }));
    const path =
      `/v1/channels/${segment(channelId)}` +
      (selection.kind === 'producer'
        ? `/sources/${selection.index}/test`
        : selection.kind === 'consumer'
          ? `/consumers/${segment(selection.id)}/test`
          : '/test');
    try {
      const value = await serviceRequest<{ first_text?: string; checks?: readonly Check[] }>(
        'event-gateway',
        path,
        { method: 'POST', signal: AbortSignal.timeout(125_000) },
      );
      setResults((values) => ({
        ...values,
        [key]: source
          ? {
              requestedVersion: channelVersion,
              text: 'Configuration check completed. External delivery was not tested.',
              checks: value.checks,
            }
          : {
              requestedVersion: channelVersion,
              text: value.first_text ?? 'The provider returned no text.',
            },
      }));
    } catch (error) {
      if (error instanceof ServiceError && [401, 403].includes(error.status)) denied(error.status);
      const uncertain = !(
        error instanceof ServiceError &&
        error.status >= 400 &&
        error.status < 500
      );
      setResults((values) => ({
        ...values,
        [key]: {
          uncertain,
          requestedVersion: channelVersion,
          text: uncertain
            ? 'Test result is unknown. Check channel activity before sending another test.'
            : error.message,
        },
      }));
    } finally {
      running.current.delete(key);
      setPending(new Set(running.current));
    }
  }
  return (
    <section className="channel-inspector__test" aria-label="Node test">
      <h3>{source ? 'Check this producer' : 'Test delivery to the agent'}</h3>
      <p>
        {source
          ? 'Checks the saved configuration and credential references. To verify incoming delivery, send a message from this platform.'
          : selection.kind === 'consumer'
            ? 'Starts a short test turn with this consumer’s saved settings. It does not send a message to the external platform.'
            : 'Starts a short test turn with the default consumer. It does not test platform ingress or external message delivery.'}
      </p>
      <Button disabled={busy} onClick={() => void test()}>
        {busy
          ? source
            ? 'Checking configuration…'
            : 'Sending test…'
          : source
            ? 'Check configuration'
            : 'Send test event'}
      </Button>
      <Presence>
        {result?.text ? (
          <Alert tone={result.uncertain ? 'warning' : 'info'} title="Test result">
            <p>{result.text}</p>
            {result.requestedVersion !== channelVersion ? (
              <p>
                Configuration changed since this test was requested. Run a new test for the current
                settings.
              </p>
            ) : null}
            {result.checks ? (
              <ul className="channel-inspector__checks">
                {result.checks.map((check) => (
                  <li key={check.id}>
                    <strong>{check.label}</strong>
                    <span>
                      {check.status === 'passed' ? 'Passed' : 'Unavailable'} · {check.detail}
                    </span>
                  </li>
                ))}
              </ul>
            ) : null}
          </Alert>
        ) : null}
      </Presence>
    </section>
  );
}
