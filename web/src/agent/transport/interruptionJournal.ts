import type { V2ResponseInterruptionRequest } from '@chronoai/cma-sdk';

export interface InterruptionRecord {
  readonly key: string;
  readonly responseId: string;
  readonly body: V2ResponseInterruptionRequest;
  readonly receipt: {
    readonly controlId: string;
    readonly commandId: string;
    readonly responseId: string;
  } | null;
}
function strict(value: unknown, fields: readonly string[]): Record<string, unknown> {
  if (typeof value !== 'object' || value === null || Array.isArray(value))
    throw new Error('Your pending interruption could not be recovered.');
  const row = value as Record<string, unknown>;
  if (
    Object.keys(row).length !== fields.length ||
    fields.some((field) => !Object.hasOwn(row, field))
  )
    throw new Error('Your pending interruption could not be recovered.');
  return row;
}
function id(value: unknown): string {
  if (
    typeof value !== 'string' ||
    !value.trim() ||
    new TextEncoder().encode(value).length > 256 ||
    [...value].some((char) => {
      const code = char.charCodeAt(0);
      return code < 32 || (code >= 127 && code <= 159);
    })
  )
    throw new Error('Invalid retained interruption identity.');
  return value;
}
function positive(value: unknown): number {
  if (typeof value !== 'number' || !Number.isSafeInteger(value) || value < 1)
    throw new Error('Invalid retained interruption target.');
  return value;
}
function parse(value: unknown): InterruptionRecord {
  const row = strict(value, ['key', 'responseId', 'body', 'receipt']);
  const body = strict(row.body, ['turn_sequence', 'expected_agent_revision', 'text']);
  if (
    typeof body.text !== 'string' ||
    !body.text.trim() ||
    new TextEncoder().encode(body.text).length > 256 * 1024
  )
    throw new Error('Invalid retained interruption text.');
  const receipt =
    row.receipt === null ? null : strict(row.receipt, ['controlId', 'commandId', 'responseId']);
  const result = {
    key: id(row.key),
    responseId: id(row.responseId),
    body: {
      turn_sequence: positive(body.turn_sequence),
      expected_agent_revision: positive(body.expected_agent_revision),
      text: body.text,
    },
    receipt:
      receipt === null
        ? null
        : {
            controlId: id(receipt.controlId),
            commandId: id(receipt.commandId),
            responseId: id(receipt.responseId),
          },
  };
  if (
    result.receipt &&
    (result.receipt.responseId === result.responseId ||
      result.receipt.commandId === result.receipt.controlId)
  )
    throw new Error('The interruption receipt does not identify a distinct new response.');
  return result;
}

/** Separate account/Agent/epoch storage leaves old input and steering intents recoverable. */
export class InterruptionJournal {
  constructor(
    private readonly storage: Pick<Storage, 'getItem' | 'setItem' | 'removeItem'>,
    private readonly key: string,
    private readonly current: () => void,
  ) {}

  read(): InterruptionRecord | null {
    this.current();
    const source = this.storage.getItem(this.key);
    return source === null ? null : parse(JSON.parse(source));
  }

  private save(intent: InterruptionRecord): void {
    this.current();
    this.storage.setItem(this.key, JSON.stringify(parse(intent)));
  }

  retain(intent: InterruptionRecord): InterruptionRecord {
    const previous = this.read();
    if (previous?.receipt === null) {
      if (
        previous.responseId !== intent.responseId ||
        JSON.stringify(previous.body) !== JSON.stringify(intent.body)
      )
        throw new Error('Recover your pending interruption before sending another input.');
      return previous;
    }
    this.save(intent);
    return this.read()!;
  }

  accepted(key: string, receipt: NonNullable<InterruptionRecord['receipt']>): void {
    const intent = this.read();
    if (
      intent?.key !== key ||
      (intent.receipt !== null && JSON.stringify(intent.receipt) !== JSON.stringify(receipt))
    )
      throw new Error('The interruption receipt changed its retained identity.');
    this.save({ ...intent, receipt });
  }

  refused(key: string): void {
    const intent = this.read();
    if (intent?.key !== key || intent.receipt !== null)
      throw new Error('The interruption refusal does not match your pending request.');
    this.current();
    this.storage.removeItem(this.key);
  }
}
