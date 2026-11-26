import { vi } from 'vitest';
import { metricsClient } from '../usage/metrics/testFixtures';
import type {
  AdminProfileSummary,
  AdminSandboxDetail,
  AdminSandboxSummary,
  PlatformSettings,
  ProfileKind,
  StoredPlatformSettings,
  WarmPoolView,
} from '@cma/client';
import type { OperatorClient } from './client';

export const now = '2026-09-06T01:00:00Z';
export const sandboxRow: AdminSandboxSummary = {
  id: 'sbx-1',
  name: 'Research Sandbox',
  owner: 'owner-1',
  owner_name: 'Alex Chen',
  status: 'active',
  runtime_version: 'runtime-2',
  agent_count: 1,
  last_activity_at: now,
  retired_at: null,
};
export const profileRow: AdminProfileSummary = {
  id: 'agp-1',
  kind: 'agent',
  name: 'Research Agent',
  owner: 'owner-1',
  owner_name: 'Alex Chen',
  visibility: 'public',
  updated_at: now,
  retired_at: null,
};
export const detail: AdminSandboxDetail = {
  owner_name: 'Alex Chen',
  sandbox: {
    id: sandboxRow.id,
    name: sandboxRow.name,
    owner: sandboxRow.owner,
    status: 'active',
    profile: { id: 'sbp-1', revision: 1, digest: 'digest' },
    runtime: { runtime_version: 'runtime-2' },
    limits: { max_workspaces: 2, max_agents_per_workspace: 2 },
    last_activity_at: now,
    created_at: now,
    updated_at: now,
    revision: 1,
  },
  workspaces: [
    {
      id: 'wks-1',
      name: 'Research Workspace',
      status: 'ready',
      profile_id: 'wkp-1',
      profile_revision: 1,
    },
  ],
  agents: [
    {
      id: 'agt-1',
      title: 'Research assistant',
      workspace_id: 'wks-1',
      worktree_id: 'wkt-1',
      status: 'ready',
      profile_id: 'agp-1',
      profile_revision: 1,
      updated_at: now,
    },
  ],
};
export const settings: PlatformSettings = {
  defaults: {
    sandbox_profile: { id: 'sbp_default', revision: 1 },
    workspace_profile: { id: 'wkp_default', revision: 2 },
    agent_profile: { id: 'agp_default', revision: 3 },
  },
  inactivity: { pause_after_seconds: 900 },
  retention: { agent_log_days: 30 },
  limits: { max_sandboxes_per_user: 10, visitor_max_sandboxes: 1, visitor_max_launches_per_day: 5 },
  triggers: { enabled: true, orchestration_paused: false },
  marketplace: { enabled: true },
  future: { preserved: true },
};
export const stored: StoredPlatformSettings = { revision: 4, settings };
export const warmPool: WarmPoolView = {
  id: 'pool-1',
  revision: 1,
  config_sha256: 'c'.repeat(64),
  image: `registry.example/cma-runtime@sha256:${'a'.repeat(64)}`,
  cpu_millis: 1000,
  memory_mib: 4096,
  target_replicas: 3,
  ready_replicas: 2,
  observed: true,
  status: 'pending',
  resource_name: 'cma-pool-1',
  error: null,
};
export const publicProfile = (kind: ProfileKind) => ({
  kind,
  id: `${kind}-public`,
  name: `Public ${kind}`,
  description: 'Published profile',
  owner_display: 'Owner',
  latest_revision: 7,
  published_at: now,
  uses_count: 1,
});
export function operatorClient() {
  const admin = {
    searchActivityActors: vi
      .fn<OperatorClient['admin']['searchActivityActors']>()
      .mockResolvedValue([]),
    listWarmPools: vi.fn<OperatorClient['admin']['listWarmPools']>().mockResolvedValue({
      items: [warmPool],
      default_cpu_millis: 1000,
      default_memory_mib: 1024,
      default_config_sha256: 'd'.repeat(64),
      default_image: warmPool.image,
      available: true,
    }),
    configureWarmPool: vi
      .fn<OperatorClient['admin']['configureWarmPool']>()
      .mockImplementation(async (body) => ({ ...warmPool, ...body, revision: 2 })),
    resizeWarmPool: vi
      .fn<OperatorClient['admin']['resizeWarmPool']>()
      .mockImplementation(async (id, body) => ({
        ...warmPool,
        id,
        target_replicas: body.target_replicas,
        revision: body.expected_revision + 1,
      })),
    listSandboxes: vi
      .fn<OperatorClient['admin']['listSandboxes']>()
      .mockResolvedValue({ items: [sandboxRow], next: null }),
    bindingHolds: vi.fn<OperatorClient['admin']['bindingHolds']>(),
    readSandbox: vi.fn<OperatorClient['admin']['readSandbox']>().mockResolvedValue(detail),
    sandboxEvents: vi.fn<OperatorClient['admin']['sandboxEvents']>().mockResolvedValue([
      {
        sandbox_id: sandboxRow.id,
        seq: 1,
        appended_at: now,
        kind: { type: 'status_changed', from: 'installing', to: 'active' },
      },
    ]),
    retireSandbox: vi
      .fn<OperatorClient['admin']['retireSandbox']>()
      .mockResolvedValue({ ...detail.sandbox, status: 'retiring' }),
    listProfiles: vi
      .fn<OperatorClient['admin']['listProfiles']>()
      .mockResolvedValue({ items: [profileRow], next: null }),
    retireProfile: vi
      .fn<OperatorClient['admin']['retireProfile']>()
      .mockResolvedValue({ ...profileRow, retired_at: now }),
    readSettings: vi.fn<OperatorClient['admin']['readSettings']>().mockResolvedValue(stored),
    readSpend: vi
      .fn<OperatorClient['admin']['readSpend']>()
      .mockResolvedValue({ metering_enabled: false, settings_revision: 1 }),
    recordSpendObservation: vi
      .fn<OperatorClient['admin']['recordSpendObservation']>()
      .mockResolvedValue({ metering_enabled: false, settings_revision: 1 }),
    replaceSettings: vi
      .fn<OperatorClient['admin']['replaceSettings']>()
      .mockImplementation(async (body) => ({
        revision: body.expected_revision + 1,
        settings: body.settings,
      })),
    previewAutoPause: vi
      .fn<OperatorClient['admin']['previewAutoPause']>()
      .mockResolvedValue({ candidates: 1 }),
    pauseIdleNow: vi.fn<OperatorClient['admin']['pauseIdleNow']>().mockResolvedValue({
      scheduled: 1,
      already_satisfied: 0,
      skipped: 0,
    }),
    setSandboxAlwaysOn: vi
      .fn<OperatorClient['admin']['setSandboxAlwaysOn']>()
      .mockImplementation(async (_id, body) => ({
        id: sandboxRow.id,
        owner: sandboxRow.owner,
        name: sandboxRow.name,
        status: sandboxRow.status,
        profile: { id: 'sbp-1', revision: 1 },
        always_on: body.always_on,
        nyxid_services: [],
        limits: { max_workspaces: 2, max_agents_per_workspace: 2 },
        lifecycle: {
          pause: { allowed: true },
          revive: { allowed: false, reason: 'not_paused' },
        },
        last_activity_at: now,
        created_at: now,
        updated_at: now,
        revision: body.expected_revision + 1,
      })),
    listActivity: vi.fn<OperatorClient['admin']['listActivity']>().mockResolvedValue({
      items: [
        {
          id: 'act-1',
          at: now,
          actor: 'operator-1',
          actor_name: 'Morgan Park',
          action: 'configure',
          target: { kind: 'resource', id: 'platform' },
          outcome: { status: 'succeeded' },
          detail: { internal: 'must not render' },
        },
      ],
      next: null,
    }),
    listActivitySummary: vi.fn<OperatorClient['admin']['listActivitySummary']>().mockResolvedValue({
      items: [
        {
          id: 'act-1',
          at: now,
          actor: 'operator-1',
          actor_name: 'Morgan Park',
          action: 'configure',
          target: { kind: 'resource', id: 'platform' },
          outcome: { status: 'succeeded' },
        },
      ],
      next: null,
    }),
  };
  return {
    readMeasuredUsage: vi.fn().mockResolvedValue(false),
    admin,
    adminUsageMetrics: metricsClient(true),
    usage: {
      summary: vi.fn<OperatorClient['usage']['summary']>(),
      sandboxes: vi.fn<OperatorClient['usage']['sandboxes']>(),
      sandbox: vi.fn<OperatorClient['usage']['sandbox']>(),
      events: vi.fn<OperatorClient['usage']['events']>(),
      adminSummary: vi.fn<OperatorClient['usage']['adminSummary']>(),
      adminSandboxes: vi.fn<OperatorClient['usage']['adminSandboxes']>(),
    },
    listPublicProfiles: vi
      .fn<OperatorClient['listPublicProfiles']>()
      .mockImplementation(async (kind) => ({ items: [publicProfile(kind)] })),
  };
}
