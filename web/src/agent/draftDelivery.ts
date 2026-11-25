import type { DraftStorage } from './draftStorage';

export type DraftBinding = 'bound' | 'superseded' | 'unavailable';

export class DraftBindingError extends Error {
  override readonly name = 'DraftBindingError';
  constructor() {
    super(
      'Your message was not sent because its draft could not be saved. Restore browser storage and retry sending.',
    );
  }
}

interface Submission {
  readonly owner: string;
  readonly agentId: string;
  readonly message: string;
  readonly bind: (key: string, storage: DraftStorage) => DraftBinding;
}
const pending = new Set<Submission>();

/** Correlate only the explicit composer submission with its transport's retained key. */
export function watchDraftSubmission(submission: Submission) {
  pending.add(submission);
  return () => {
    pending.delete(submission);
  };
}

export function bindDraftCommand(
  owner: string,
  agentId: string,
  message: string,
  key: string,
  storage: DraftStorage,
): DraftBinding | 'untracked' {
  let result: DraftBinding | 'untracked' = 'untracked';
  for (const submission of pending) {
    if (
      submission.owner === owner &&
      submission.agentId === agentId &&
      submission.message === message
    ) {
      const binding = submission.bind(key, storage);
      if (result !== 'unavailable') result = binding;
      pending.delete(submission);
    }
  }
  return result;
}
