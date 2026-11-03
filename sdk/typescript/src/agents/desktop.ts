import { enumeration, integer, object, optional, text, type Output } from '../schema.js';
import { query, read } from '../resources.js';
import type { ResolvedConfig } from '../config.js';
import type { OperationOptions } from '../http.js';

export const desktopCapability = object({
  state: enumeration('available', 'unsupported', 'temporarily_unavailable'),
  reason: text,
  runtime_contract: integer,
  min_memory_mib: integer,
  min_cpu_millis: integer,
  runtime_image: optional(text),
});
export type DesktopCapability = Output<typeof desktopCapability>;
export interface DesktopScope {
  readonly workspace_id?: string;
  readonly sandbox_id?: string;
}
export function readDesktopCapability(
  config: ResolvedConfig,
  scope: DesktopScope,
  options: OperationOptions,
) {
  return read(
    config,
    query('/api/v1/desktop-capability', { ...scope }),
    desktopCapability,
    options,
  );
}
