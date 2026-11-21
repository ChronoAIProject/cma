import { useEffect, useRef, useState } from 'react';
import { ApiError } from '@cma/client';
import { CommandKeys, type CommandOptions } from '../api/client';

export interface PublisherFailure {
  readonly message: string;
  readonly field?: string;
  readonly uncertain: boolean;
  readonly conflict: boolean;
}
export function publisherFailure(error: unknown): PublisherFailure {
  if (!(error instanceof ApiError))
    return {
      message: 'The outcome could not be confirmed. Retry the original request.',
      uncertain: true,
      conflict: false,
    };
  const conflict = error.code === 'trigger_draft_changed';
  const uncertain =
    error.status >= 500 ||
    error.status === 408 ||
    error.code === 'idempotency_in_flight' ||
    error.code === 'idempotency_recovery_required';
  const messages: Readonly<Record<string, string>> = {
    trigger_draft_changed: 'The saved draft changed. Your changes are still here.',
    trigger_reference_not_public:
      'Every referenced profile must be public before this Trigger can be published.',
    trigger_reference_unavailable:
      'This profile revision is no longer available. Choose another revision.',
    forbidden: 'You do not have permission to publish Triggers.',
    trigger_retired: 'This Trigger is retired.',
  };
  return {
    message:
      messages[error.code ?? ''] ??
      (uncertain
        ? 'The outcome could not be confirmed. Retry the original request.'
        : error.message),
    field: error.field,
    uncertain,
    conflict,
  };
}

/** The retained closure pins both the original payload and every key in a multi-step command. */
export function usePublisherCommand<Result>(
  onSuccess: (result: Result) => void,
  onFailure?: (failure: PublisherFailure) => void,
) {
  const [pending, setPending] = useState(false);
  const [failure, setFailure] = useState<PublisherFailure>();
  const active = useRef<AbortController | undefined>(undefined);
  const retained = useRef<((options: CommandOptions) => Promise<Result>) | undefined>(undefined);
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
        idempotencyKey: keys.current.for('command'),
        signal: request.signal,
      });
      if (request.signal.aborted) return;
      retained.current = undefined;
      keys.current.settle('command');
      onSuccess(result);
    } catch (error) {
      if (request.signal.aborted) return;
      const next = publisherFailure(error);
      setFailure(next);
      onFailure?.(next);
      if (!next.uncertain) {
        retained.current = undefined;
        keys.current.settle('command');
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
    clearFailure: () => setFailure(undefined),
    retry: () => {
      if (retained.current !== undefined) void submit(retained.current);
    },
  };
}
