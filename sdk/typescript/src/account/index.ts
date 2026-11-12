import type { ResolvedConfig } from '../config.js';
import { request, type OperationOptions } from '../http.js';
import { command, read, type CommandOptions } from '../resources.js';
import {
  array,
  dictionary,
  enumeration,
  integer,
  nullable,
  object,
  text,
  type Output,
} from '../schema.js';

const erasureStep = enumeration(
  'inventory',
  'retire_sandboxes',
  'release_homes',
  'erase_records',
  'erase_objects',
  'revoke_credentials',
  'verify_no_residue',
);
/** Ordered phase of the server's account erasure state machine. */
export type ErasureStep = Output<typeof erasureStep>;

const stepProgress = object({
  step: erasureStep,
  completed_at: text,
  counts: dictionary(integer),
});
/** Recorded completion evidence for one erasure phase. */
export type AccountDeletionStepProgress = Output<typeof stepProgress>;

const receipt = object({
  id: text,
  status: enumeration('running', 'blocked', 'completed'),
  step: erasureStep,
  requested_at: text,
  deadline_at: text,
  completed_at: nullable(text),
  steps: array(stepProgress),
});
/** Durable deletion acceptance and progress; acceptance alone does not mean completion. */
export type AccountDeletionReceipt = Output<typeof receipt>;

/** Explicit owner-only irreversible deletion commands and progress reads. */
export class AccountDeletion {
  constructor(private readonly config: ResolvedConfig) {}

  /** Acknowledge irreversible CMA data deletion using the caller's retained replay key. */
  async request(options: CommandOptions): Promise<AccountDeletionReceipt> {
    return receipt(
      await request(this.config, '/api/v1/account/deletion', {
        ...command(options),
        method: 'POST',
        body: { acknowledge: 'irreversible' },
        expectedStatus: 202,
      }),
    );
  }

  /** Read the current receipt; a failed read never implies that deletion completed. */
  status(options: OperationOptions = {}): Promise<AccountDeletionReceipt> {
    return read(this.config, '/api/v1/account/deletion', receipt, options);
  }
}

/** Human owner's account operations. */
export class Account {
  readonly deletion: AccountDeletion;

  constructor(config: ResolvedConfig) {
    this.deletion = new AccountDeletion(config);
  }
}
