import type { DraftStorage } from './draftStorage';

interface Submission {
  readonly owner: string;
  readonly agentId: string;
  readonly message: string;
  readonly bind: (key: string, storage: DraftStorage) => void;
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
) {
  for (const submission of pending) {
    if (
      submission.owner === owner &&
      submission.agentId === agentId &&
      submission.message === message
    ) {
      submission.bind(key, storage);
      pending.delete(submission);
    }
  }
}
