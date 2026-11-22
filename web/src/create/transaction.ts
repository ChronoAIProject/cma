import { useEffect, useRef, useState } from 'react';
import { CommandKeys, type CommandOptions } from '../api/client';
import { creationFailure, type CreationFailure } from './failure';

/** The retained closure owns both the immutable body and the command key. */
export function useCreationCommand<Result>(onCreated: (result: Result) => void) {
  const [pending, setPending] = useState(false);
  const [failure, setFailure] = useState<CreationFailure>();
  const active = useRef<AbortController | undefined>(undefined);
  const retained = useRef<((options: CommandOptions) => Promise<Result>) | undefined>(undefined);
  const unresolved = useRef(false);
  const keys = useRef(new CommandKeys());
  useEffect(() => () => active.current?.abort(), []);
  const submit = async (operation: (options: CommandOptions) => Promise<Result>) => {
    if (active.current !== undefined) return;
    retained.current ??= operation;
    const request = new AbortController();
    active.current = request;
    setPending(true);
    setFailure(undefined);
    try {
      const result = await retained.current({
        idempotencyKey: keys.current.for('create'),
        signal: request.signal,
      });
      if (request.signal.aborted) return;
      keys.current.settle('create');
      retained.current = undefined;
      unresolved.current = false;
      onCreated(result);
    } catch (error: unknown) {
      if (request.signal.aborted) return;
      const problem = creationFailure(error, unresolved.current);
      unresolved.current = problem.uncertain;
      setFailure(problem);
      if (!problem.uncertain) {
        retained.current = undefined;
        keys.current.settle('create');
      }
    } finally {
      active.current = undefined;
      if (!request.signal.aborted) setPending(false);
    }
  };
  return {
    pending,
    failure,
    locked: pending || failure?.uncertain === true,
    submit,
    retry: () => {
      if (retained.current !== undefined) void submit(retained.current);
    },
  };
}
