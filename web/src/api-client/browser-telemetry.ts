import type { ResolvedConfig } from './config.js';
import { requestVoid, type OperationOptions } from './http.js';

/** Fixed, content-free browser diagnostics; these reports are untrusted. */
export type BrowserFault =
  'uncaught_error' | 'unhandled_rejection' | 'react_error' | 'resource_error' | 'chunk_error';

/** Send one anonymous diagnostic without credentials or automatic retry. */
export function reportBrowserFault(
  config: ResolvedConfig,
  kind: BrowserFault,
  options: OperationOptions = {},
): Promise<void> {
  return requestVoid(
    { ...config, credential: undefined, cookies: false },
    '/api/v1/telemetry/browser',
    {
      ...options,
      method: 'POST',
      body: { kind },
      expectedStatus: 204,
    },
  );
}
