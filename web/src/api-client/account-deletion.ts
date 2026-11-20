import type { ResolvedConfig } from './config.js';
import type { OperationOptions } from './http.js';
import { command, input, read, type CommandOptions } from './resources.js';
import {
  array,
  dictionary,
  enumeration,
  integer,
  nullable,
  object,
  optional,
  text,
  type Output,
} from './schema.js';

const step = enumeration(
  'inventory',
  'retire_sandboxes',
  'release_homes',
  'erase_records',
  'erase_objects',
  'revoke_credentials',
  'verify_no_residue',
);
const receipt = object({
  id: text,
  status: enumeration('running', 'blocked', 'completed'),
  step,
  requested_at: text,
  deadline_at: text,
  completed_at: nullable(text),
  steps: array(object({ step, completed_at: text, counts: dictionary(integer) })),
});
const deletionRequest = object({ acknowledge: optional(nullable(text)) });
export type AccountDeletionReceipt = Output<typeof receipt>;
export interface AccountDeletionRequest {
  readonly acknowledge: 'irreversible';
}

export class AccountDeletion {
  constructor(private readonly config: ResolvedConfig) {}

  status(options: OperationOptions = {}): Promise<AccountDeletionReceipt> {
    return read(this.config, '/api/v1/account/deletion', receipt, options);
  }

  request(body: AccountDeletionRequest, options: CommandOptions): Promise<AccountDeletionReceipt> {
    input(deletionRequest, body);
    return read(this.config, '/api/v1/account/deletion', receipt, {
      ...command(options),
      method: 'POST',
      expectedStatus: 202,
      body,
    });
  }
}
