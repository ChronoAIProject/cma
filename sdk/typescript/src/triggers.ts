import type { ResolvedConfig } from './config.js';
import type { OperationOptions } from './http.js';
import { read, segment } from './resources.js';
import { triggerPreview } from './launch/types.js';

export * from './trigger-authoring/index.js';

/** Public preview never resolves a bearer credential or sends browser cookies. */
export function readTriggerPreview(
  config: ResolvedConfig,
  publicId: string,
  options: OperationOptions = {},
) {
  return read(
    { ...config, credential: undefined, cookies: false },
    `/api/v1/triggers/public/${segment(publicId)}`,
    triggerPreview,
    options,
  );
}
