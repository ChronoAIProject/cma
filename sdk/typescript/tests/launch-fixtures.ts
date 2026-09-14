import type { Launch, TriggerPreview } from '../src/launch/index.js';

export const launchFixture: Launch = {
  id: 'lch_example',
  trigger_id: 'trg_example',
  revision_digest: 'review-one',
  public_id: 'public-example',
  state: 'draft',
  steps: [{ key: 'checking_requirements', status: 'completed', safe_detail: 'Accounts checked' }],
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
export const previewFixture: TriggerPreview = {
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
