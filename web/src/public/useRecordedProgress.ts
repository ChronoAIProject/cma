import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { ProtocolError, type Launch } from '@cma/client';
import { refusalSentence } from '../vocabulary/errors';
import { publicCopy } from '../vocabulary/needs';
import type { PublicClient } from './client';

function stopped(record: Launch): boolean {
  return ['draft', 'ready', 'completed', 'failed_terminal', 'cancelled'].includes(record.state);
}

interface ProgressScope {
  client: PublicClient;
  id: string;
}

export function useRecordedProgress(client: PublicClient, id: string) {
  const scope = useMemo<ProgressScope>(() => ({ client, id }), [client, id]);
  const observed = useRef<{ scope: ProgressScope; record?: Launch } | undefined>(undefined);
  const [snapshot, setSnapshot] = useState<{
    scope: ProgressScope;
    record: Launch;
    observedAt: string;
  }>();
  const [failure, setFailure] = useState<{ scope: ProgressScope; message: string }>();
  const [revision, setRevision] = useState(0);
  useEffect(() => {
    observed.current = { scope };
    return () => {
      if (observed.current?.scope === scope) observed.current = undefined;
    };
  }, [scope]);
  const setError = useCallback(
    (message?: string) => {
      if (observed.current?.scope === scope)
        setFailure(message === undefined ? undefined : { scope, message });
    },
    [scope],
  );
  const observe = useCallback(
    (value: Launch) => {
      if (value.id !== id)
        throw new ProtocolError('The recorded progress belongs to another request');
      const prior = observed.current?.record;
      if (prior && (value.revision < prior.revision || value.event_head < prior.event_head))
        return false;
      observed.current = { scope, record: value };
      setSnapshot({ scope, record: value, observedAt: new Date().toISOString() });
      setError(undefined);
      return true;
    },
    [id, scope, setError],
  );
  const refresh = useCallback(() => {
    if (observed.current?.scope === scope) setRevision((value) => value + 1);
  }, [scope]);
  const changed = useCallback(
    (value: Launch) => {
      if (observed.current?.scope !== scope) return;
      try {
        observe(value);
      } catch (cause) {
        setError(refusalSentence(cause));
      }
      refresh();
    },
    [scope, observe, setError, refresh],
  );
  useEffect(() => {
    const controller = new AbortController();
    let timer: ReturnType<typeof setTimeout> | undefined;
    let cursor = observed.current?.record?.event_head ?? 0;
    let latest: Launch | undefined;
    let failures = 0;
    let outageBeganAt: number | undefined;
    const read = async (minimum = cursor) => {
      const value = await client.requests.read(id, { signal: controller.signal });
      if (controller.signal.aborted) return;
      if (value.event_head < minimum)
        throw new ProtocolError('The recorded progress has not reached the delivered event');
      if (!observe(value))
        throw new ProtocolError('The recorded progress is older than the last observation');
      latest = value;
      cursor = value.event_head;
    };
    const poll = async () => {
      try {
        await read();
        failures = 0;
        outageBeganAt = undefined;
      } catch (cause) {
        if (controller.signal.aborted) return;
        failures += 1;
        outageBeganAt ??= Date.now();
        setError(refusalSentence(cause));
      }
      if (controller.signal.aborted || (latest !== undefined && stopped(latest))) return;
      // Healthy preparation can outlast five minutes; bound only uninterrupted read outages.
      const remaining =
        outageBeganAt === undefined ? 300_000 : 300_000 - (Date.now() - outageBeganAt);
      if (remaining <= 0) {
        setError(publicCopy.updatesPaused);
        return;
      }
      timer = setTimeout(() => void poll(), Math.min(3000 * 2 ** failures, 15_000, remaining));
    };
    const follow = async () => {
      try {
        await read();
        if (controller.signal.aborted || latest === undefined || stopped(latest)) return;
        for (let attempt = 0; attempt < 2; attempt += 1) {
          try {
            await client.requests.stream(id, {
              signal: AbortSignal.any([controller.signal, AbortSignal.timeout(45_000)]),
              after: cursor,
              onEvent: async (event) => {
                if (event.seq <= cursor) return;
                await read(event.seq);
                if (latest !== undefined && stopped(latest)) controller.abort();
              },
            });
          } catch (cause) {
            if (controller.signal.aborted) return;
            setError(refusalSentence(cause));
          }
          if (controller.signal.aborted) return;
        }
      } catch (cause) {
        if (controller.signal.aborted) return;
        setError(refusalSentence(cause));
      }
      if (!controller.signal.aborted) timer = setTimeout(() => void poll(), 3000);
    };
    void follow();
    return () => {
      controller.abort();
      clearTimeout(timer);
    };
  }, [client, id, revision, scope, observe, setError]);
  return {
    record: snapshot?.scope === scope ? snapshot.record : undefined,
    observedAt: snapshot?.scope === scope ? snapshot.observedAt : undefined,
    error: failure?.scope === scope ? failure.message : undefined,
    refresh,
    changed,
  };
}
