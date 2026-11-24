import type { ResolvedConfig } from './config.js';
import type { OperationOptions } from './http.js';
import { input, read } from './resources.js';
import {
  runtimeCanaryPolicy,
  runtimeCanaryPolicyRequest,
  type RuntimeCanaryPolicyRequest,
} from './runtime-canary.js';
import { boolean, integer, object, text, type Output } from './schema.js';

const proposal = object({
  sandbox_id: text,
  owner: text,
  sandbox_revision: integer,
  release_key: text,
  policy_revision: integer,
  journal_recovery: boolean,
});
const preview = object({
  predecessor_image: text,
  predecessor_generation: integer,
  predecessor_journal_recovery: boolean,
  target_image: text,
  target_journal_recovery: boolean,
  plan_sha256: text,
});
const grantRequest = object({
  proposal,
  grant_id: text,
  plan_sha256: text,
  consent_sha256: text,
  custody_sha256: text,
  expires_at: text,
});
const grant = object({ grant_id: text });
const abortRequest = object({ owner: text, grant_id: text });
const redemption = object({ grant_id: text });
const receipt = object({ operation_id: text, successor_generation: integer });

export type RetainedRuntimeProposal = Output<typeof proposal>;
export type RetainedRuntimePreview = Output<typeof preview>;
export type RetainedRuntimeGrantRequest = Output<typeof grantRequest>;
export type RetainedRuntimeGrant = Output<typeof grant>;
export type RetainedRuntimeAbortRequest = Output<typeof abortRequest>;
export type RetainedRuntimeRedemption = Output<typeof redemption>;
export type RetainedRuntimeReceipt = Output<typeof receipt>;

export class RetainedRuntimeTransitions {
  constructor(private readonly config: ResolvedConfig) {}

  setPolicy(body: RuntimeCanaryPolicyRequest, options: OperationOptions = {}) {
    return read(this.config, '/api/v1/admin/retained-runtime-policy', runtimeCanaryPolicy, {
      ...options,
      method: 'PUT',
      body: input(runtimeCanaryPolicyRequest, body),
    });
  }

  preview(body: RetainedRuntimeProposal, options: OperationOptions = {}) {
    return read(this.config, '/api/v1/admin/retained-runtime-preview', preview, {
      ...options,
      method: 'POST',
      body: input(proposal, body),
    });
  }

  grant(body: RetainedRuntimeGrantRequest, options: OperationOptions = {}) {
    return read(this.config, '/api/v1/admin/retained-runtime-grants', grant, {
      ...options,
      method: 'POST',
      body: input(grantRequest, body),
      expectedStatus: 201,
    });
  }

  abort(body: RetainedRuntimeAbortRequest, options: OperationOptions = {}) {
    return read(this.config, '/api/v1/admin/retained-runtime-grants/abort', grant, {
      ...options,
      method: 'POST',
      body: input(abortRequest, body),
    });
  }

  redeem(body: RetainedRuntimeRedemption, options: OperationOptions = {}) {
    return read(this.config, '/api/v1/retained-runtime-redemptions', receipt, {
      ...options,
      method: 'POST',
      body: input(redemption, body),
      expectedStatus: 202,
    });
  }
}
