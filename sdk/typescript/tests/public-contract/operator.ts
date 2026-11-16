import type { ProfileKind } from '../../src/index.js';
import type { Dispatch } from './context.js';

export const operatorDispatch: Record<string, Dispatch> = {
  'GET /api/v1/profile-authoring/bounds': ({ client }) => client.profileAuthoring.bounds(),
  'GET /api/v1/profile-authoring/services': ({ client }) => client.profileAuthoring.services(),
  'GET /api/v1/profile-authoring/skills': ({ client, options }) =>
    client.profileAuthoring.searchSkills(options),
  'POST /api/v1/profile-authoring/repositories/resolve': ({ client, body }) =>
    client.profileAuthoring.resolveRepository(body),
  'GET /api/v1/admin/settings': ({ client }) => client.admin.readSettings(),
  'PUT /api/v1/admin/admission/ceilings': ({ client, body }) =>
    client.admin.replaceAdmissionCeilings(body),
  'GET /api/v1/admin/admission': ({ client }) => client.admin.readAdmission(),
  'POST /api/v1/admin/admission/stop': ({ client, body }) => client.admin.stopAdmission(body),
  'POST /api/v1/admin/admission/resume': ({ client, body }) => client.admin.resumeAdmission(body),
  'PUT /api/v1/admin/settings': ({ client, body }) => client.admin.replaceSettings(body),
  'PATCH /api/v1/admin/settings': ({ client, body }) => client.admin.patchSettings(body),
  'POST /api/v1/admin/settings/reset': ({ client, body }) => client.admin.resetSettings(body),
  'GET /api/v1/admin/auto-pause/preview': ({ client }) => client.admin.previewAutoPause(),
  'POST /api/v1/admin/auto-pause/pause-idle-now': ({ client, body }) =>
    client.admin.pauseIdleNow((body as { confirmed: boolean }).confirmed),
  'POST /api/v1/admin/sandboxes/{id}/always-on': ({ client, param, body }) =>
    client.admin.setSandboxAlwaysOn(param('id'), body),
  'POST /api/v1/admin/triggers/pause': ({ client }) => client.admin.pauseTriggers(),
  'POST /api/v1/admin/triggers/resume': ({ client }) => client.admin.resumeTriggers(),
  'GET /api/v1/admin/sandboxes': ({ client, options }) => {
    const values = options as { always_on?: string };
    return client.admin.listSandboxes({
      ...values,
      always_on: values.always_on === undefined ? undefined : values.always_on === 'true',
    });
  },
  'GET /api/v1/admin/sandboxes/{id}': ({ client, param }) => client.admin.readSandbox(param('id')),
  'GET /api/v1/admin/sandboxes/{id}/events': ({ client, param, options }) =>
    client.admin.sandboxEvents(param('id'), options),
  'POST /api/v1/admin/sandboxes/{id}/retire': ({ client, param }) =>
    client.admin.retireSandbox(param('id')),
  'POST /api/v1/admin/sandboxes/{id}/runtime-update': ({ client, param, body }) =>
    client.admin.updateSandboxRuntime(param('id'), body),
  'GET /api/v1/admin/profiles': ({ client, options }) => client.admin.listProfiles(options),
  'POST /api/v1/admin/profiles/{kind}/{id}/retire': ({ client, param }) =>
    client.admin.retireProfile(param('kind') as ProfileKind, param('id')),
  'GET /api/v1/admin/runtime-inventory': ({ client }) => client.admin.readRuntimeInventory(),
  'GET /api/v1/admin/warm-pools': ({ client }) => client.admin.listWarmPools(),
  'POST /api/v1/admin/warm-pools': ({ client, body, command }) =>
    client.admin.configureWarmPool(body, command),
  'PUT /api/v1/admin/warm-pools/{id}': ({ client, param, body }) =>
    client.admin.resizeWarmPool(param('id'), body),
  'GET /api/v1/admin/activity': ({ client, options }) => client.admin.listActivity(options),
  'GET /auth/session': ({ client }) => client.auth.readSession(),
  'GET /auth/signup-options': ({ client }) => client.auth.readSignupOptions(),
  'POST /api/v1/mcp': ({ client }) => client.unsupported.mcp(),
  'POST /api/v1/a2a': ({ client }) => client.unsupported.a2a(),
  'GET /.well-known/agent-card.json': ({ client }) => client.unsupported.agentCard(),
};
