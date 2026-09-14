import { unavailableClient, type CmaWebClient } from '../api/client';

export function testClient(overrides: Partial<CmaWebClient> = {}): CmaWebClient {
  return { ...unavailableClient, ...overrides };
}
