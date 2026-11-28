import { ApiError, type Cma } from '@cma/client';
import type { ReadOptions } from '../client';
import { workspaceDetail } from './hierarchy';

export class WorkspaceReadUnavailable extends ApiError {
  constructor(error: ApiError) {
    super(
      503,
      'Workspace status is temporarily unavailable (HTTP 503).',
      error.code,
      error.requestId,
      error.retryAfterMs,
      error.extensions,
      error.statusLink,
      error.problemType,
    );
  }
}

export async function readWorkspaceDetail(sdk: Cma, id: string, options?: ReadOptions) {
  try {
    return workspaceDetail(await sdk.workspaces.read(id, options));
  } catch (error) {
    if (error instanceof ApiError && error.status === 503)
      throw new WorkspaceReadUnavailable(error);
    throw error;
  }
}
