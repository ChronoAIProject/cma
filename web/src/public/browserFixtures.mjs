const now = '2026-09-07T00:00:00Z';
const first = 'Inspect this project.\nKeep my formatting.  ';
const preview = {
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
  first_message: first,
};
const initial = {
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
  requirements: { services: [], checked_at: now },
  review: { needs: [], can_start_now: true },
  trust: { version: 1, disclosures: [] },
  first_message: 'queued',
  confirmations: [],
  event_head: 1,
  created_at: now,
  updated_at: now,
  revision: 1,
};
const trust = {
  ...initial,
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
const ready = {
  ...initial,
  state: 'ready',
  event_head: 2,
  resolution: {
    sandbox: { state: 'created', id: 'sbx_one' },
    workspace: { state: 'created', id: 'wsp_one' },
    agent: { state: 'created', id: 'agt_ready' },
    choice_required: false,
  },
};
const ref = { id: 'profile-one', revision: 1 };
const agent = {
  id: 'agt_ready',
  status: 'ready',
  profile: {
    ...ref,
    digest: 'profile-version',
    model: 'gpt-5',
    thinking_effort: 'high',
    permission_level: 'guarded',
    skills: [],
  },
  worktree: {
    id: 'tree-one',
    sandbox_id: 'sbx_one',
    workspace_id: 'wsp_one',
    name: 'Research',
    path: '/worktrees/research',
    branch: 'research',
    base_commit: 'commit-one',
    status: 'ready',
    agent_id: 'agt_ready',
  },
  workspace: {
    id: 'wsp_one',
    name: 'project',
    path: '/workspaces/project',
    status: 'ready',
    profile: { ...ref, digest: 'workspace-version' },
    resolved_commit: 'commit-one',
  },
  sandbox: {
    id: 'sbx_one',
    name: 'Research',
    status: 'active',
    profile: { ...ref, digest: 'sandbox-version' },
  },
  conforms_to: ['https://spec.chrono-ai.fun/cma/1.0/conf/conversation'],
  reach: [],
  title: 'Project research',
  first_message: { state: 'completed', queued_at: now },
  created_at: now,
  updated_at: now,
  ready_at: now,
};
const workspace = {
  id: 'wsp_one',
  sandbox_id: 'sbx_one',
  name: 'project',
  path: '/workspaces/project',
  status: 'ready',
  profile: ref,
  source: { type: 'empty' },
  created_at: now,
  revision: 1,
  worktrees: [agent.worktree],
  agents: [],
  worktree_count: 1,
  agent_count: 1,
};

export { now, first, preview, initial, trust, ready, agent, workspace };
