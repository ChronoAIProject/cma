// Component fixtures for already connected users. ConnectionWizard's own
// tests exercise missing, cancelled, stale and sequential service connections.
import { beforeEach, vi } from 'vitest';
import { connectionClient } from '../connections/client';

beforeEach(() => {
  vi.spyOn(connectionClient, 'check').mockResolvedValue({
    owner: 'owner',
    ready: true,
    checked_at: '2026-09-11T00:00:00Z',
    services: [
      {
        service: 'chrono-llm-public',
        display_name: 'Chrono LLM (public)',
        kind: 'codex',
        state: 'satisfied',
        connection_mode: 'hosted',
      },
    ],
  });
});
