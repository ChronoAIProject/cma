import type { Cma, Launches } from '@chronoai/cma-sdk';

export interface PublicClient {
  readonly requests: Pick<
    Launches,
    | 'preview'
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
