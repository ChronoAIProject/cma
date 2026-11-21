import { AVAILABLE_CAPACITY } from './capacity';
import { unavailableClient, type CmaWebClient } from '../api/client';

export function testClient(overrides: Partial<CmaWebClient> = {}): CmaWebClient {
  return {
    ...unavailableClient,
    readSandboxCapacity: async () => AVAILABLE_CAPACITY,
    ...overrides,
  };
}
