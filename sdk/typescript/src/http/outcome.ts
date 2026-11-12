import type { ResolvedConfig } from '../config.js';
import { readJsonBody, requireSuccess, send, withDeadline, type RequestOptions } from '../http.js';

/** Read explicitly admitted wire statuses under the same bounded request deadline. */
export async function requestOutcome(
  config: ResolvedConfig,
  path: string,
  options: RequestOptions & { readonly expectedStatus: readonly number[] },
): Promise<{ response: Response; body: unknown }> {
  return withDeadline(config, options, async (signal, requestId) => {
    const { response, metadata } = await send(config, path, options, signal, requestId);
    if (!options.expectedStatus.includes(response.status))
      await requireSuccess(response, options.expectedStatus, signal, metadata.requestId);
    return { response, body: await readJsonBody(response, signal) };
  });
}
