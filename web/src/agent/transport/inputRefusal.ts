import type { InputRecord } from './journalRecord';

export function responseBody(input: InputRecord) {
  return {
    input: input.input,
    stream: true as const,
    ...(input.answerContext ? { answer_context: input.answerContext } : {}),
  };
}

export function sameInput(current: InputRecord | null, expected: InputRecord): boolean {
  return (
    current?.key === expected.key &&
    JSON.stringify(responseBody(current)) === JSON.stringify(responseBody(expected))
  );
}

/** The storage key includes the owner, Agent and sign-in epoch. */
export function inputRefusalJournal(
  storage: Pick<Storage, 'getItem' | 'setItem' | 'removeItem'>,
  key: string,
  readInput: () => InputRecord | null,
) {
  const cooldownKey = `${key}:retry-at`;
  const evidence = (input: InputRecord) =>
    JSON.stringify({ key: input.key, body: responseBody(input) });
  const matches = (input: InputRecord) => {
    const current = readInput();
    return (
      current?.state === 'pending' &&
      sameInput(current, input) &&
      storage.getItem(key) === evidence(input)
    );
  };

  return {
    remember(input: InputRecord, retryAfterMs?: number) {
      const current = readInput();
      if (current?.state !== 'pending' || !sameInput(current, input)) return false;
      // Persist the deadline before evidence can authorize journal release.
      if (retryAfterMs !== undefined)
        storage.setItem(cooldownKey, String(Date.now() + retryAfterMs));
      storage.setItem(key, evidence(input));
      return true;
    },
    matches,
    clear(input: InputRecord) {
      readInput();
      // The settled input's evidence is already unusable even if cleanup fails.
      try {
        if (storage.getItem(key) === evidence(input)) storage.removeItem(key);
      } catch {
        /* Keep the durable settlement. */
      }
    },
    retryAt() {
      readInput();
      const deadline = Number(storage.getItem(cooldownKey));
      return Number.isSafeInteger(deadline) && deadline > Date.now() ? deadline : undefined;
    },
  };
}
