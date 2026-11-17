import type { ResolvedConfig } from './config.js';
import type { OperationOptions } from './http.js';
import { profileReference } from './profiles/types.js';
import { read } from './resources.js';
import { object, type Output } from './schema.js';

const defaults = object({
  sandbox_profile: profileReference,
  workspace_profile: profileReference,
  agent_profile: profileReference,
});

export type PlatformDefaults = Output<typeof defaults>;

/** Current deployment choices, projected without private platform settings. */
export function readDefaults(config: ResolvedConfig, options: OperationOptions = {}) {
  return read(config, '/api/v1/defaults', defaults, options);
}
