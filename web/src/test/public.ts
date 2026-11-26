import { vi } from 'vitest';
import type { Launch, TriggerPreview } from '@cma/client';
import type { PublicClient } from '../public/client';
import type { User } from '../auth/types';

export const PUBLIC_USER: User = {
  id: 'visitor-one',
  handle: 'Ada',
  visitor: true,
  capabilities: ['launch_triggers'],
};
export const PUBLIC_PREVIEW: TriggerPreview = {
  title: 'Research assistant',
  description: 'Investigate the project with your own agent.',
  button_label: 'open_in_cma',
  theme: 'auto',
  publisher_display_name: 'ChronoAI',
  what_happens: ['CMA prepares your sandbox.', 'Your agent reads the project.'],
  availability: 'published',
  trust: { disclosures: [] },
  technical: {
    provider: 'codex',
    effort: 'high',
    size: { cpu_millis: 2000, memory_mib: 4096 },
    services: [],
  },
  first_message: 'Inspect this project.\nKeep my formatting.  ',
};
export const PUBLIC_RECORD: Launch = {
  id: 'lch_example',
  trigger_id: 'trg_example',
  revision_digest: 'review-one',
  public_id: 'public-example',
  state: 'resolving',
  steps: [
    { key: 'signing_in', status: 'completed', safe_detail: 'Signed in' },
    { key: 'checking_requirements', status: 'completed', safe_detail: 'Accounts checked' },
    { key: 'sandbox', status: 'running', safe_detail: 'Preparing software' },
  ],
  resolution: {
    sandbox: { state: 'pending' },
    workspace: { state: 'pending' },
    agent: { state: 'pending' },
    choice_required: false,
  },
  requirements: { services: [], checked_at: '2026-09-06T00:00:00Z' },
  review: { needs: [], can_start_now: true },
  trust: { version: 1, disclosures: [] },
  first_message: 'queued',
  confirmations: [],
  event_head: 1,
  created_at: '2026-09-06T00:00:00Z',
  updated_at: '2026-09-06T00:00:00Z',
  revision: 1,
};
export const PUBLIC_TRUST: Launch = {
  ...PUBLIC_RECORD,
  state: 'draft',
  review: {
    can_start_now: false,
    needs: [{ type: 'trust_confirmation', commands_digest: 'approved-one' }],
  },
  trust: {
    version: 1,
    confirmation_digest: 'approved-one',
    disclosures: [
      {
        resource: 'sandbox',
        name: 'Research software',
        commands_digest: 'install-one',
        commands: ['printf "keep_this_value"\n'],
        environment_variables: [{ name: 'PROJECT_PATH', value: '/home/cma/project_data' }],
      },
    ],
  },
};
export function publicClient(): PublicClient {
  return {
    requests: {
      preview: vi.fn().mockResolvedValue(PUBLIC_PREVIEW),
      entryIntent: vi.fn().mockResolvedValue({ accepted: false }),
      createOrResumeBound: vi.fn().mockResolvedValue(PUBLIC_RECORD),
      read: vi.fn().mockResolvedValue(PUBLIC_RECORD),
      start: vi.fn().mockResolvedValue(PUBLIC_RECORD),
      cancel: vi.fn().mockResolvedValue({ ...PUBLIC_RECORD, state: 'cancelled' }),
      requirements: vi.fn().mockResolvedValue(PUBLIC_RECORD.requirements),
      events: vi
        .fn()
        .mockResolvedValue({ launch_id: PUBLIC_RECORD.id, events: [], next_cursor: 1 }),
      stream: vi.fn().mockImplementation(() => new Promise(() => undefined)),
      createConnection: vi.fn().mockResolvedValue({
        handoff_id: 'handoff-one',
        hosted_url: 'https://identity.example.test/connect',
        expires_at: '2026-09-06T00:10:00Z',
      }),
      readConnection: vi.fn().mockResolvedValue({
        handoff_id: 'handoff-one',
        launch_id: PUBLIC_RECORD.id,
        service_id: 'code',
        state: 'satisfied',
        expires_at: '2026-09-06T00:10:00Z',
        updated_at: PUBLIC_RECORD.updated_at,
      }),
    },
  };
}
