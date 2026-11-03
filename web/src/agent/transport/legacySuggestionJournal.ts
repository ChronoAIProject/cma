import { answerContext, type AnswerContext } from '@chronoai/cma-sdk';

export interface LegacySuggestionRecord {
  readonly key: string;
  readonly input: string;
  readonly answerContext: AnswerContext;
  readonly state: 'pending' | 'accepted';
  readonly receipt: { readonly runId: string } | null;
}

export class LegacySuggestionJournal {
  constructor(
    private readonly storage: Pick<Storage, 'getItem' | 'setItem'>,
    private readonly key: string,
    private readonly current: () => void,
    private readonly makeKey: () => string,
  ) {}

  read(): LegacySuggestionRecord | null {
    this.current();
    const source = this.storage.getItem(this.key);
    if (source === null) return null;
    const value: unknown = JSON.parse(source);
    if (value === null) return null;
    if (typeof value !== 'object' || Array.isArray(value))
      throw new Error('Pending suggestion is invalid.');
    const row = value as Record<string, unknown>;
    if (
      Object.keys(row).length !== 5 ||
      typeof row.key !== 'string' ||
      !/^[A-Za-z0-9-]{1,128}$/.test(row.key) ||
      typeof row.input !== 'string' ||
      !row.input.trim() ||
      new TextEncoder().encode(row.input).length > 256 * 1024 ||
      !['pending', 'accepted'].includes(String(row.state))
    )
      throw new Error('Pending suggestion is invalid.');
    const scope = answerContext(row.answerContext);
    if (row.state === 'pending' && row.receipt === null)
      return {
        key: row.key,
        input: row.input,
        answerContext: scope,
        state: 'pending',
        receipt: null,
      };
    const receipt = row.receipt as { runId?: unknown } | null;
    if (
      row.state !== 'accepted' ||
      !receipt ||
      Object.keys(receipt).length !== 1 ||
      typeof receipt.runId !== 'string' ||
      !/^\d+$/.test(receipt.runId)
    )
      throw new Error('Pending suggestion receipt is invalid.');
    return {
      key: row.key,
      input: row.input,
      answerContext: scope,
      state: 'accepted',
      receipt: { runId: receipt.runId },
    };
  }

  retain(input: string, scope: AnswerContext): LegacySuggestionRecord {
    const previous = this.read();
    if (previous?.state === 'pending') {
      if (
        previous.input !== input ||
        JSON.stringify(previous.answerContext) !== JSON.stringify(scope)
      )
        throw new Error('Recover your pending suggestion before sending another message.');
      return previous;
    }
    this.storage.setItem(
      this.key,
      JSON.stringify({
        key: this.makeKey(),
        input,
        answerContext: answerContext(scope),
        state: 'pending',
        receipt: null,
      }),
    );
    return this.read()!;
  }

  accepted(key: string, runId: string): void {
    const current = this.read();
    if (!current || current.key !== key || (current.receipt && current.receipt.runId !== runId))
      throw new Error('Suggestion receipt changed identity.');
    this.storage.setItem(
      this.key,
      JSON.stringify({ ...current, state: 'accepted', receipt: { runId } }),
    );
  }

  refused(key: string): void {
    const current = this.read();
    if (current?.key !== key || current.state !== 'pending')
      throw new Error('Suggestion refusal changed identity.');
    this.storage.setItem(this.key, 'null');
  }
}
