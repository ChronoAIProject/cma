import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import type { SandboxDetail } from '@chronoai/cma-sdk';
import { localOwner } from './eventGateway';

/*
 * Local Agent fixtures: one Sandbox (`sbx_route`) holding one Workspace
 * (`wks_route`) and one Agent (`agt_contract`) with a finished two-turn
 * conversation. Resource bodies are the recorded public-contract responses the
 * LOCAL browser checks also use (`e2e/agent-startup-fixtures.ts`), so their
 * shape follows the API. Node-only: read by the dev server, never bundled.
 */

const contract = '../../../api/crates/layer1-public/tests/fixtures/public-contract/';

function recorded<T>(file: string, id: string): T {
  const corpus = JSON.parse(
    readFileSync(fileURLToPath(new URL(`${contract}${file}`, import.meta.url)), 'utf8'),
  ) as { cases: { id: string; response: { body: T } }[] };
  const entry = corpus.cases.find((item) => item.id === id);
  if (!entry) throw new Error(`Missing public-contract fixture ${file}#${id}`);
  return entry.response.body;
}

export const agentId = 'agt_contract';
const sandboxId = 'sbx_route';
const workspaceId = 'wks_route';

type Kind = Record<string, unknown> & { readonly type: string };
interface Step {
  readonly at: string;
  readonly kind: Kind;
}

const conversation: readonly Step[][] = [
  [
    { at: '2026-10-29T01:00:00Z', kind: { type: 'turn_started', turn_sequence: 1 } },
    {
      at: '2026-10-29T01:00:00Z',
      kind: {
        type: 'owner_message',
        content: 'Summarize the repository layout and tell me how to run the tests.',
      },
    },
    {
      at: '2026-10-29T01:00:02Z',
      kind: {
        type: 'tool_call_started',
        call_id: 'read-readme',
        title: 'Read README.md',
        kind: 'read',
        status: 'in_progress',
      },
    },
    {
      at: '2026-10-29T01:00:03Z',
      kind: { type: 'tool_call_updated', call_id: 'read-readme', status: 'completed' },
    },
    {
      at: '2026-10-29T01:00:04Z',
      kind: {
        type: 'tool_call_started',
        call_id: 'run-tests',
        title: 'npm test',
        kind: 'execute',
        status: 'in_progress',
      },
    },
    {
      at: '2026-10-29T01:00:19Z',
      kind: { type: 'tool_call_updated', call_id: 'run-tests', status: 'completed' },
    },
    {
      at: '2026-10-29T01:00:21Z',
      kind: {
        type: 'agent_message',
        content: [
          'The project has three parts:',
          '',
          '- `api/` — the Rust backend',
          '- `web/` — the React client',
          '- `docs/` — contracts and operations notes',
          '',
          'Run the frontend tests from the repository root:',
          '',
          '```sh',
          'npm test --prefix web',
          '```',
          '',
          'All **1,932** tests passed in 15 seconds.',
        ].join('\n'),
      },
    },
    { at: '2026-10-29T01:00:21Z', kind: { type: 'turn_completed', turn_sequence: 1 } },
  ],
  [
    { at: '2026-10-29T01:05:00Z', kind: { type: 'turn_started', turn_sequence: 2 } },
    {
      at: '2026-10-29T01:05:00Z',
      kind: { type: 'owner_message', content: 'Add a test for an empty channel list.' },
    },
    {
      at: '2026-10-29T01:05:02Z',
      kind: {
        type: 'tool_call_started',
        call_id: 'edit-test',
        title: 'Edit ChannelList.test.tsx',
        kind: 'edit',
        status: 'in_progress',
      },
    },
    {
      at: '2026-10-29T01:05:06Z',
      kind: { type: 'tool_call_updated', call_id: 'edit-test', status: 'completed' },
    },
    {
      at: '2026-10-29T01:05:08Z',
      kind: {
        type: 'agent_message',
        content:
          'Added `renders the empty state when no channels exist` to `ChannelList.test.tsx`. It passes locally.',
      },
    },
    { at: '2026-10-29T01:05:08Z', kind: { type: 'turn_completed', turn_sequence: 2 } },
  ],
];

/** Durable Agent events, numbered from 1 like the API. */
export const agentEvents = conversation.flat().map((step, index) => ({
  seq: index + 1,
  appended_at: step.at,
  kind: step.kind,
}));
const throughSeq = agentEvents.length;

let sequence = 0;
const transcript = {
  agent_id: agentId,
  through_seq: throughSeq,
  turns: conversation.map((turn, index) => {
    const messages = turn.flatMap((step) => {
      sequence += 1;
      if (step.kind.type !== 'owner_message' && step.kind.type !== 'agent_message') return [];
      return [
        {
          sequence,
          role: step.kind.type === 'owner_message' ? 'user' : 'assistant',
          content: step.kind.content,
          appended_at: step.at,
        },
      ];
    });
    return {
      turn_sequence: index + 1,
      started_at: turn[0]!.at,
      owner_message: messages.find((message) => message.role === 'user')?.content ?? '',
      agent_messages: messages
        .filter((message) => message.role === 'assistant')
        .map((message) => message.content),
      messages,
      outcome: { type: 'completed', at: turn.at(-1)!.at },
    };
  }),
};

/** Exact-path reads for the Agent pages. */
export function agentReads(): Record<string, unknown> {
  const sandbox = {
    ...recorded<SandboxDetail>('instances.json', 'sandbox-read'),
    owner: localOwner.subject,
  };
  const workspace = recorded<{ worktrees: unknown[]; agents: unknown[] }>(
    'instances.json',
    'workspace-read',
  );
  const agent = recorded<Record<string, unknown>>('agents.json', 'agent-read');
  const agentPath = `/api/v1/agents/${agentId}`;
  return {
    '/api/v1/sandboxes': { items: [sandbox] },
    [`/api/v1/sandboxes/${sandboxId}`]: sandbox,
    [`/api/v1/sandboxes/${sandboxId}/workspaces`]: { items: sandbox.workspaces },
    [`/api/v1/sandboxes/${sandboxId}/events`]: [],
    [`/api/v1/workspaces/${workspaceId}`]: workspace,
    [`/api/v1/workspaces/${workspaceId}/worktrees`]: { items: workspace.worktrees },
    '/api/v1/agents': { items: workspace.agents },
    [agentPath]: agent,
    [`${agentPath}/provenance`]: {
      agent_id: agentId,
      created_at: agent.created_at,
      creation_origin: 'legacy_unknown',
    },
    [`${agentPath}/source`]: { agent_id: agentId, origin: { type: 'manual' } },
    [`${agentPath}/channel-usage`]: {
      agent_id: agentId,
      channel_usage: { active: false, has_history: false, first_used_at: null, last_used_at: null },
    },
    [`${agentPath}/resources`]: {
      agent_id: agentId,
      sandbox_id: sandboxId,
      uptime_ms: 3_600_000,
      cpu_usage_usec: 42_000_000,
      cpu_millis_limit: 4000,
      memory_bytes_used: 734_003_200,
      memory_bytes_limit: 8_589_934_592,
      memory_bytes_peak: null,
      stall_permille: null,
      ceiling_hits: null,
      allocation_failures: null,
      kills: null,
      received_bytes: 18_432,
      sent_bytes: 9_216,
    },
    [`${agentPath}/transcript`]: transcript,
    [`${agentPath}/usage`]: {
      agent_id: agentId,
      through_seq: throughSeq,
      input_tokens: 25_061,
      output_tokens: 846,
      context_used: 25_907,
    },
    [`${agentPath}/message-feedback`]: { items: [], next: null },
    [`${agentPath}/models`]: { models: [{ id: 'gpt-6', label: 'GPT-6' }] },
    [`${agentPath}/skills`]: { items: [], pending: false, can_install: true },
    [`/api/agent-manager/v1/agents/${agentId}/access-requests`]: { items: [] },
    [`/api/agent-manager/v1/agents/${agentId}/settings`]: {
      model: 'gpt-6',
      reasoning_effort: 'high',
      permission_mode: 'autonomous',
      pending: false,
    },
    [`/api/v1/resource-access/agent/${agentId}`]: {
      capabilities: ['view_details', 'view_history', 'manage_sharing'],
      owner: { kind: 'person', is_current_person: true },
      revalidate_at: null,
      policy_revision: 1,
    },
    [`/api/v1/resource-access/sandbox/${sandboxId}`]: {
      capabilities: ['view_details', 'view_history', 'manage_sharing'],
      owner: { kind: 'person', is_current_person: true },
      revalidate_at: null,
      policy_revision: 1,
    },
    '/api/v1/agent-profiles': { items: [] },
    '/api/v1/workspace-profiles': { items: [] },
    '/api/v1/defaults': {
      sandbox_profile: { id: 'sbp_route', revision: 1 },
      workspace_profile: { id: 'wkp_route', revision: 1 },
      agent_profile: { id: 'agp_route', revision: 1 },
    },
    '/api/v1/profile-authoring/services': [
      { id: 'api-github', catalog_id: 'catalog-github', name: 'GitHub' },
    ],
  };
}

/** `GET /api/v1/{kind}-profiles/{id}[/revisions/{n}]`, any id. */
export function profileRead(path: string): unknown {
  const match =
    /^\/api\/v1\/(sandbox|workspace|agent)-profiles\/([^/]+)(?:\/revisions\/(\d+))?$/.exec(path);
  if (!match) return undefined;
  const [, kind, id, revision] = match;
  return revision === undefined
    ? { ...recorded<object>('profiles.json', `${kind}-profile-read`), id, name: `${kind} profile` }
    : {
        ...recorded<object>('profiles.json', `${kind}-profile-exact-revision`),
        profile_id: id,
        revision: Number(revision),
      };
}

/** `GET /api/v1/agents/{id}/events?after=n`. */
export function eventsAfter(after: number) {
  return { items: agentEvents.filter((event) => event.seq > after) };
}
