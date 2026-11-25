import { useChannelWriteGuard } from './useChannelWriteGuard';
import { useRef, useState } from 'react';
import { Alert } from '../ui/Alert';
import { Button } from '../ui/Button';
import { Presence } from '../ui/Presence';
import { segment, ServiceError, serviceRequest, type Denied } from './client';
import { selectionKey, type ChannelSelection } from './channelTopology';

type Result = {
  text: string;
  uncertain?: boolean;
  completed?: boolean;
  providerText?: string;
  acceptedAfterMs?: number;
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
  writable = true,
  channelVersion,
  selection,
  denied,
}: {
  readonly channelId: string;
  readonly writable?: boolean;
  readonly channelVersion?: number;
  readonly selection: ChannelSelection;
  readonly denied: Denied;
}) {
  const canWrite = useChannelWriteGuard(writable);
  const key = selectionKey(selection);
  const [results, setResults] = useState<Record<string, Result>>({});
  const [pending, setPending] = useState<ReadonlySet<string>>(new Set());
  const running = useRef(new Set<string>());
  const source = selection.kind === 'producer';
  const result = results[key];
  const busy = pending.has(key);
  async function test() {
    if (!canWrite() || running.current.has(key)) return;
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
      const value = await serviceRequest<{
        first_text?: string;
        accepted_after_ms?: number;
        checks?: readonly Check[];
      }>('event-gateway', path, { method: 'POST', signal: AbortSignal.timeout(125_000) });
      if (!source && typeof value?.first_text !== 'string') {
        throw new Error('Invalid channel test response');
      }
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
              completed: true,
              text: 'Test turn completed',
              providerText: value.first_text?.trim(),
              acceptedAfterMs: value.accepted_after_ms,
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
      <Button disabled={!writable || busy} onClick={() => void test()}>
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
          <Alert
            tone={result.uncertain ? 'warning' : result.completed ? 'success' : 'info'}
            title={result.completed ? result.text : 'Test result'}
          >
            {result.completed ? (
              <>
                {typeof result.acceptedAfterMs === 'number' &&
                Number.isFinite(result.acceptedAfterMs) &&
                result.acceptedAfterMs >= 0 ? (
                  <p>Accepted after {result.acceptedAfterMs} ms.</p>
                ) : null}
                {result.providerText ? (
                  <>
                    <p>Provider response</p>
                    <p>{result.providerText}</p>
                  </>
                ) : (
                  <p>The provider returned no text.</p>
                )}
              </>
            ) : (
              <p>{result.text}</p>
            )}
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
