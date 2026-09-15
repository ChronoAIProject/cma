import type { V2ResponseControlRequest } from '@chronoai/cma-sdk';

export interface ControlRecord {
  readonly key: string;
  readonly responseId: string;
  readonly body: V2ResponseControlRequest;
  readonly commandId: string | null;
}

function strict(value: unknown, fields: readonly string[]): Record<string, unknown> {
  if (typeof value !== 'object' || value === null || Array.isArray(value))
    throw new Error('Invalid retained action.');
  const row = value as Record<string, unknown>;
  if (
    Object.keys(row).length !== fields.length ||
    fields.some((field) => !Object.hasOwn(row, field))
  )
    throw new Error('Invalid retained action.');
  return row;
}
function id(value: unknown): string {
  if (typeof value !== 'string' || !value.trim() || value.length > 512)
    throw new Error('Invalid retained action identity.');
  return value;
}
function positive(value: unknown): number {
  if (typeof value !== 'number' || !Number.isSafeInteger(value) || value < 1)
    throw new Error('Invalid retained action target.');
  return value;
}
export function parseControl(value: unknown): ControlRecord {
  const row = strict(value, ['key', 'responseId', 'body', 'commandId']);
  const body = strict(row.body, ['turn_sequence', 'expected_agent_revision', 'command']);
  const command = body.command as Record<string, unknown> | null;
  let intent: V2ResponseControlRequest['command'];
  switch (command?.type) {
    case 'cancel':
      strict(command, ['type']);
      intent = { type: 'cancel' };
      break;
    case 'steer':
      strict(command, ['type', 'text']);
      if (
        typeof command.text !== 'string' ||
        !command.text.trim() ||
        new TextEncoder().encode(command.text).length > 256 * 1024
      )
        throw new Error('Invalid retained steering message.');
      intent = { type: 'steer', text: command.text };
      break;
    case 'resolve_approval':
      strict(command, ['type', 'approval_id', 'decision']);
      if (!['allow', 'deny', 'abort'].includes(String(command.decision)))
        throw new Error('Invalid retained approval decision.');
      intent = {
        type: 'resolve_approval',
        approval_id: id(command.approval_id),
        decision: command.decision as 'allow' | 'deny' | 'abort',
      };
      break;
    default:
      throw new Error('Invalid retained action.');
  }
  return {
    key: id(row.key),
    responseId: id(row.responseId),
    commandId: row.commandId === null ? null : id(row.commandId),
    body: {
      turn_sequence: positive(body.turn_sequence),
      expected_agent_revision: positive(body.expected_agent_revision),
      command: intent,
    },
  };
}
