import type { SandboxCapacity } from '@cma/client';

export const AVAILABLE_CAPACITY: SandboxCapacity = {
  resource: 'sandbox',
  limit: 10,
  live: 1,
  available: 9,
  counts_by_status: { active: 1 },
  observed_at: '2026-11-19T12:00:00Z',
};
