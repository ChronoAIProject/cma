import { useCallback, useEffect, useState } from 'react';
import { ProtocolError, type Launch } from '@chronoai/cma-sdk';
import { refusalSentence } from '../vocabulary/errors';
import { publicCopy } from '../vocabulary/needs';
import type { PublicClient } from './client';

function stopped(record: Launch): boolean {
  return ['draft', 'ready', 'completed', 'failed_terminal', 'cancelled'].includes(record.state);
}

export function useRecordedProgress(client: PublicClient, id: string) {
  const [record, setRecord] = useState<Launch>();
  const [error, setError] = useState<string>();
  const [observedAt, setObservedAt] = useState<string>();
  const [revision, setRevision] = useState(0);
  const refresh = useCallback(() => setRevision((value) => value + 1), []);
  const changed = useCallback((value: Launch) => {
    setRecord(value);
    setObservedAt(new Date().toISOString());
    setError(undefined);
    setRevision((prior) => prior + 1);
  }, []);
  useEffect(() => {
    const controller = new AbortController();
    let timer: ReturnType<typeof setTimeout> | undefined;
    let cursor = 0;
    let latest: Launch | undefined;
    let failures = 0;
    const beganAt = Date.now();
    const read = async (minimum = cursor) => {
      const value = await client.requests.read(id, { signal: controller.signal });
      if (controller.signal.aborted) return;
      if (value.event_head < minimum)
        throw new ProtocolError('The recorded progress has not reached the delivered event');
      latest = value;
      cursor = value.event_head;
      setRecord(value);
      setObservedAt(new Date().toISOString());
      setError(undefined);
    };
    const poll = async () => {
      try {
        await read();
        failures = 0;
      } catch (cause) {
        if (controller.signal.aborted) return;
        failures += 1;
        setError(refusalSentence(cause));
      }
      if (controller.signal.aborted || (latest !== undefined && stopped(latest))) return;
      const remaining = 300_000 - (Date.now() - beganAt);
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
  }, [client, id, revision]);
  return { record, error, observedAt, refresh, changed };
}
