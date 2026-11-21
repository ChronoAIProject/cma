import type { Cma, Launches } from '@cma/client';

export interface PublicClient {
  readonly requests: Pick<
    Launches,
    | 'preview'
    | 'entryIntent'
    | 'createOrResume'
    | 'read'
    | 'start'
    | 'cancel'
    | 'requirements'
    | 'events'
    | 'stream'
    | 'createConnection'
    | 'readConnection'
  >;
}

export function createPublicClient(sdk: Cma): PublicClient {
  return { requests: sdk.launches };
}
