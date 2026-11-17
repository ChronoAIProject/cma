import type { V2QueuedCancellationRequest } from '@cma/client';

export interface QueuedCancellationRecord {
  readonly key: string;
  readonly responseId: string;
  readonly body: V2QueuedCancellationRequest;
  readonly commandId: string | null;
}

function strict(value: unknown, fields: readonly string[]): Record<string, unknown> {
  if (typeof value !== 'object' || value === null || Array.isArray(value))
    throw new Error('Your pending cancellation could not be recovered.');
  const row = value as Record<string, unknown>;
  if (Object.keys(row).length !== fields.length || fields.some((key) => !Object.hasOwn(row, key)))
    throw new Error('Your pending cancellation could not be recovered.');
  return row;
}
function identifier(value: unknown): string {
  if (
    typeof value !== 'string' ||
    !value.trim() ||
    new TextEncoder().encode(value).length > 256 ||
    [...value].some((char) => {
      const code = char.charCodeAt(0);
      return code < 32 || (code >= 127 && code <= 159);
    })
  )
    throw new Error('Your pending cancellation has an invalid identity.');
  return value;
}
function parse(value: unknown): QueuedCancellationRecord {
  const row = strict(value, ['key', 'responseId', 'body', 'commandId']);
  const body = strict(row.body, ['source_command_id', 'expected_command_revision']);
  const revision = body.expected_command_revision;
  if (typeof revision !== 'number' || !Number.isSafeInteger(revision) || revision < 1)
    throw new Error('Your pending cancellation has an invalid target.');
  return {
    key: identifier(row.key),
    responseId: identifier(row.responseId),
    commandId: row.commandId === null ? null : identifier(row.commandId),
    body: {
      source_command_id: identifier(body.source_command_id),
      expected_command_revision: revision,
    },
  };
}

/** Separate storage preserves the existing input/control journal wire shape. */
export class QueuedCancellationJournal {
  constructor(
    private readonly storage: Pick<Storage, 'getItem' | 'setItem' | 'removeItem'>,
    private readonly key: string,
    private readonly agent: string,
    private readonly epoch: string,
    private readonly current: () => void,
  ) {}

  read(): QueuedCancellationRecord | null {
    this.current();
    const source = this.storage.getItem(this.key);
    if (source === null) return null;
    const row = strict(JSON.parse(source), ['agent', 'epoch', 'intent']);
    if (row.agent !== this.agent || row.epoch !== this.epoch)
      throw new Error('Your pending cancellation changed its conversation identity.');
    return parse(row.intent);
  }

  private save(intent: QueuedCancellationRecord): void {
    this.current();
    this.storage.setItem(
      this.key,
      JSON.stringify({
        agent: this.agent,
        epoch: this.epoch,
        intent: parse(intent),
      }),
    );
  }

  retain(intent: QueuedCancellationRecord): QueuedCancellationRecord {
    const prior = this.read();
    if (prior?.commandId === null) {
      if (
        prior.responseId !== intent.responseId ||
        JSON.stringify(prior.body) !== JSON.stringify(intent.body)
      )
        throw new Error('Recover your pending cancellation before cancelling another message.');
      return prior;
    }
    this.save(intent);
    return this.read()!;
  }

  accepted(key: string, commandId: string): void {
    const intent = this.read();
    if (
      intent === null ||
      intent.key !== key ||
      (intent.commandId !== null && intent.commandId !== commandId)
    )
      throw new Error('The cancellation receipt does not match your retained request.');
    this.save({ ...intent, commandId });
  }

  refused(key: string): void {
    const intent = this.read();
    if (intent?.key !== key || intent.commandId !== null)
      throw new Error('The cancellation refusal does not match your pending request.');
    this.current();
    this.storage.removeItem(this.key);
  }
}
