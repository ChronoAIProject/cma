import { agentLifecycleCreateReceipt } from './agents/lifecycle.js';
import type { ResolvedConfig } from './config.js';
import type { OperationOptions } from './http.js';
import { command, input, read, type CommandOptions } from './resources.js';
import { boolean, integer, object, text, type Output } from './schema.js';

export const runtimeCanaryPolicyRequest = object({ expected_revision: integer, open: boolean });
export const runtimeCanaryPolicy = object({ revision: integer });
const grantRequest = object({
  owner: text,
  release_key: text,
  expected_revision: integer,
  policy_revision: integer,
  consent_sha256: text,
  expires_at: text,
});
const grant = object({ grant_id: text, revision: integer, policy_revision: integer });
const revokeRequest = object({
  owner: text,
  release_key: text,
  grant_id: text,
  expected_revision: integer,
});
const redemptionRequest = object({
  release_key: text,
  grant_id: text,
  policy_revision: integer,
  name: text,
  initial_message: text,
});

export type RuntimeCanaryPolicyRequest = Output<typeof runtimeCanaryPolicyRequest>;
export type RuntimeCanaryPolicy = Output<typeof runtimeCanaryPolicy>;
export type RuntimeCanaryGrantRequest = Output<typeof grantRequest>;
export type RuntimeCanaryGrant = Output<typeof grant>;
export type RuntimeCanaryRevokeRequest = Output<typeof revokeRequest>;
export type RuntimeCanaryRedemptionRequest = Output<typeof redemptionRequest>;

export class RuntimeCanaries {
  constructor(private readonly config: ResolvedConfig) {}

  setPolicy(body: RuntimeCanaryPolicyRequest, options: OperationOptions = {}) {
    return read(this.config, '/api/v1/admin/runtime-canary-policy', runtimeCanaryPolicy, {
      ...options,
      method: 'PUT',
      body: input(runtimeCanaryPolicyRequest, body),
    });
  }

  grant(body: RuntimeCanaryGrantRequest, options: OperationOptions = {}) {
    return read(this.config, '/api/v1/admin/runtime-canary-grants', grant, {
      ...options,
      method: 'POST',
      body: input(grantRequest, body),
      expectedStatus: 201,
    });
  }

  revoke(body: RuntimeCanaryRevokeRequest, options: OperationOptions = {}) {
    return read(this.config, '/api/v1/admin/runtime-canary-grants/revoke', grant, {
      ...options,
      method: 'POST',
      body: input(revokeRequest, body),
    });
  }

  redeem(body: RuntimeCanaryRedemptionRequest, options: CommandOptions) {
    return read(this.config, '/api/v1/runtime-canary-redemptions', agentLifecycleCreateReceipt, {
      ...command(options),
      method: 'POST',
      body: input(redemptionRequest, body),
      expectedStatus: 202,
    });
  }
}
