export interface QueuedInputIntent {
  readonly key: string;
  readonly responseId: string;
  readonly commandId: string;
  readonly revision: number;
  readonly original: string;
  readonly action: 'edit' | 'remove';
  readonly text?: string;
}

function parse(value: unknown): QueuedInputIntent {
  if (value === null || typeof value !== 'object') throw new Error('Invalid queued request.');
  const row = value as Record<string, unknown>;
  const id = (value: unknown) =>
    typeof value === 'string' &&
    !!value.trim() &&
    new TextEncoder().encode(value).length <= 256 &&
    [...value].every((char) => {
      const code = char.charCodeAt(0);
      return code > 32 && (code < 127 || code > 159);
    });
  const text = (value: unknown) =>
    typeof value === 'string' &&
    !!value.trim() &&
    new TextEncoder().encode(value).length <= 256 * 1024;
  if (
    !id(row.key) ||
    !id(row.responseId) ||
    !id(row.commandId) ||
    !Number.isSafeInteger(row.revision) ||
    Number(row.revision) < 1 ||
    !text(row.original) ||
    !['edit', 'remove'].includes(String(row.action)) ||
    (row.action === 'edit' ? !text(row.text) : row.text !== undefined) ||
    Object.keys(row).some(
      (key) =>
        !['key', 'responseId', 'commandId', 'revision', 'original', 'action', 'text'].includes(key),
    )
  )
    throw new Error('Your queued request could not be recovered.');
  return row as unknown as QueuedInputIntent;
}

/** Independent durable intents: uncertainty for one queue entry never replaces another. */
export class QueuedInputJournal {
  constructor(
    private readonly storage: Pick<Storage, 'getItem' | 'setItem'>,
    private readonly key: string,
    private readonly current: () => void,
  ) {}

  managedCommands(): ReadonlySet<string> {
    this.current();
    const source = this.storage.getItem(`${this.key}:managed`);
    if (source === null) return new Set();
    const value: unknown = JSON.parse(source);
    if (
      !Array.isArray(value) ||
      value.some((entry) => typeof entry !== 'string' || !entry || entry.length > 256)
    )
      throw new Error('Your queued message status could not be recovered.');
    return new Set(value as string[]);
  }

  /** Known command IDs seed status reconciliation after reload; absence is not an outcome. */
  remember(commandIds: readonly string[]): void {
    const known = new Set(this.managedCommands());
    for (const id of commandIds) known.add(id);
    this.storage.setItem(`${this.key}:managed`, JSON.stringify([...known]));
  }

  read(): readonly QueuedInputIntent[] {
    return this.readEntries(this.key);
  }

  unappliedEdits(): readonly QueuedInputIntent[] {
    return this.readEntries(`${this.key}:unapplied`);
  }

  private readEntries(key: string): readonly QueuedInputIntent[] {
    this.current();
    const source = this.storage.getItem(key);
    if (source === null) return [];
    const value: unknown = JSON.parse(source);
    if (!Array.isArray(value)) throw new Error('Your queued requests could not be recovered.');
    const entries = value.map(parse);
    if (new Set(entries.map((entry) => entry.responseId)).size !== entries.length)
      throw new Error('Your queued requests contain duplicate targets.');
    return entries;
  }

  retain(intent: QueuedInputIntent): QueuedInputIntent {
    const entries = this.read();
    const previous = entries.find((entry) => entry.responseId === intent.responseId);
    if (previous) {
      if (
        previous.action !== intent.action ||
        previous.commandId !== intent.commandId ||
        previous.revision !== intent.revision ||
        previous.text !== intent.text
      )
        throw new Error('Confirm the pending request for this message before changing it.');
      return previous;
    }
    this.storage.setItem(this.key, JSON.stringify([...entries, parse(intent)]));
    this.remember([intent.commandId]);
    return this.read().find((entry) => entry.key === intent.key)!;
  }

  settle(intent: QueuedInputIntent, keepDraft = false): void {
    const entries = this.read();
    const previous = entries.find((entry) => entry.responseId === intent.responseId);
    if (previous?.key !== intent.key)
      throw new Error('The queued receipt changed request identity.');
    if (keepDraft) {
      const drafts = this.unappliedEdits().filter(
        (entry) => entry.responseId !== intent.responseId,
      );
      // Preserve the text before clearing recovery so storage failure leaves the exact request.
      this.storage.setItem(`${this.key}:unapplied`, JSON.stringify([...drafts, previous]));
    }
    this.storage.setItem(
      this.key,
      JSON.stringify(entries.filter((entry) => entry.key !== intent.key)),
    );
  }

  dismissUnappliedEdit(key: string): void {
    const drafts = this.unappliedEdits();
    this.storage.setItem(
      `${this.key}:unapplied`,
      JSON.stringify(drafts.filter((entry) => entry.key !== key)),
    );
  }
}
