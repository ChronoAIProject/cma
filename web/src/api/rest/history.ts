import { ProtocolError, type Cma } from '@cma/client';
import { readJsonBody } from '../../api-client/http';
import type { ReadOptions } from '../client';

const explanations: Record<string, string> = {
  history_export_unavailable: 'Raw history is currently unavailable from the runtime.',
  history_export_unsupported: 'This runtime does not support raw history export.',
  history_export_rejected: 'The runtime could not provide a valid raw history export.',
  runtime_request_capacity_exhausted: 'The runtime is busy. Retry raw history export later.',
  instance_service_unavailable: 'Raw history is currently unavailable from the runtime.',
};

export async function exportRawHistory(sdk: Cma, id: string, options?: ReadOptions): Promise<Blob> {
  const response = await sdk.config.fetch(
    `${sdk.config.baseUrl}/api/v1/agents/${encodeURIComponent(id)}/history`,
    {
      credentials: sdk.config.cookies ? 'include' : 'omit',
      cache: 'no-store',
      signal: options?.signal,
    },
  );
  if (!response.ok) {
    let code: string | undefined;
    try {
      const problem: unknown = await readJsonBody(response, options?.signal);
      if (
        problem &&
        typeof problem === 'object' &&
        'code' in problem &&
        typeof problem.code === 'string' &&
        Object.hasOwn(explanations, problem.code)
      )
        code = problem.code;
    } catch {
      // Error documents are optional; never expose raw upstream content.
    }
    const reason = code ? explanations[code] : `Raw history export failed (${response.status}).`;
    const recovery =
      code === 'history_export_unsupported'
        ? ' Use Export retained conversation for recorded messages.'
        : response.status >= 500 || response.status === 429
          ? ' Retry later, or use Export retained conversation for recorded messages without starting the runtime.'
          : ' Refresh to recheck your access before retrying.';
    throw new ProtocolError(`${reason}${code ? ` (${code})` : ''}${recovery}`);
  }
  return response.blob();
}
