import { agentLifecycleCreateReceipt } from './agents/lifecycle.js';
import type { ResolvedConfig } from './config.js';
import type { OperationOptions } from './http.js';
import { command, input, read, type CommandOptions } from './resources.js';
import { boolean, integer, object, text, type Output } from './schema.js';

export const runtimeCanaryPolicyRequest = object({ expected_revision: integer, open: boolean });
export const runtimeCanaryPolicyView = object({ revision: integer });
const grantTarget = { owner: text, release_key: text, expected_revision: integer };
const grantRequest = object({
  ...grantTarget,
  policy_revision: integer,
  consent_sha256: text,
  expires_at: text,
});
const revokeRequest = object({ ...grantTarget, grant_id: text });
const grantView = object({ grant_id: text, revision: integer, policy_revision: integer });
const redeemRequest = object({
  grant_id: text,
  release_key: text,
  policy_revision: integer,
  name: text,
  initial_message: text,
});

export type RuntimeCanaryPolicyRequest = Output<typeof runtimeCanaryPolicyRequest>;
export type RuntimeCanaryGrantRequest = Output<typeof grantRequest>;
export type RuntimeCanaryRevokeRequest = Output<typeof revokeRequest>;
export type RuntimeCanaryRedeemRequest = Output<typeof redeemRequest>;

export class RuntimeCanaries {
  constructor(private readonly config: ResolvedConfig) {}

  configure(body: RuntimeCanaryPolicyRequest, options: OperationOptions = {}) {
    return read(this.config, '/api/v1/admin/runtime-canary-policy', runtimeCanaryPolicyView, {
      ...options,
      method: 'PUT',
      body: input(runtimeCanaryPolicyRequest, body),
    });
  }

  grant(body: RuntimeCanaryGrantRequest, options: OperationOptions = {}) {
    return read(this.config, '/api/v1/admin/runtime-canary-grants', grantView, {
      ...options,
      method: 'POST',
      body: input(grantRequest, body),
      expectedStatus: 201,
    });
  }

  revoke(body: RuntimeCanaryRevokeRequest, options: OperationOptions = {}) {
    return read(this.config, '/api/v1/admin/runtime-canary-grants/revoke', grantView, {
      ...options,
      method: 'POST',
      body: input(revokeRequest, body),
    });
  }

  redeem(body: RuntimeCanaryRedeemRequest, options: CommandOptions) {
    return read(this.config, '/api/v1/runtime-canary-redemptions', agentLifecycleCreateReceipt, {
      ...command(options),
      method: 'POST',
      body: input(redeemRequest, body),
      expectedStatus: 202,
    });
  }
}
