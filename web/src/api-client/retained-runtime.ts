import type { ResolvedConfig } from './config.js';
import type { OperationOptions } from './http.js';
import { input, read } from './resources.js';
import {
  runtimeCanaryPolicyRequest,
  runtimeCanaryPolicyView,
  type RuntimeCanaryPolicyRequest,
} from './runtime-canaries.js';
import { boolean, integer, object, text, type Output } from './schema.js';

const proposal = object({
  sandbox_id: text,
  owner: text,
  sandbox_revision: integer,
  release_key: text,
  policy_revision: integer,
  journal_recovery: boolean,
});
const previewView = object({
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
const grantView = object({ grant_id: text });
const abortRequest = object({ owner: text, grant_id: text });
const redeemRequest = object({ grant_id: text });
const receipt = object({ operation_id: text, successor_generation: integer });

export type RetainedRuntimeProposal = Output<typeof proposal>;
export type RetainedRuntimeGrantRequest = Output<typeof grantRequest>;
export type RetainedRuntimeAbortRequest = Output<typeof abortRequest>;
export type RetainedRuntimeRedeemRequest = Output<typeof redeemRequest>;

export class RetainedRuntime {
  constructor(private readonly config: ResolvedConfig) {}

  configure(body: RuntimeCanaryPolicyRequest, options: OperationOptions = {}) {
    return read(this.config, '/api/v1/admin/retained-runtime-policy', runtimeCanaryPolicyView, {
      ...options,
      method: 'PUT',
      body: input(runtimeCanaryPolicyRequest, body),
    });
  }

  preview(body: RetainedRuntimeProposal, options: OperationOptions = {}) {
    return read(this.config, '/api/v1/admin/retained-runtime-preview', previewView, {
      ...options,
      method: 'POST',
      body: input(proposal, body),
    });
  }

  grant(body: RetainedRuntimeGrantRequest, options: OperationOptions = {}) {
    return read(this.config, '/api/v1/admin/retained-runtime-grants', grantView, {
      ...options,
      method: 'POST',
      body: input(grantRequest, body),
      expectedStatus: 201,
    });
  }

  abort(body: RetainedRuntimeAbortRequest, options: OperationOptions = {}) {
    return read(this.config, '/api/v1/admin/retained-runtime-grants/abort', grantView, {
      ...options,
      method: 'POST',
      body: input(abortRequest, body),
    });
  }

  redeem(body: RetainedRuntimeRedeemRequest, options: OperationOptions = {}) {
    return read(this.config, '/api/v1/retained-runtime-redemptions', receipt, {
      ...options,
      method: 'POST',
      body: input(redeemRequest, body),
      expectedStatus: 202,
    });
  }
}
