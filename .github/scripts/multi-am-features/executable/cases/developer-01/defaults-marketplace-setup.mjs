import {randomUUID} from 'node:crypto';
import {requireResourceInputs} from '../developer-17/setup-resources.mjs';
import {required, request} from './common.mjs';
import {writableSettings} from './public-setups.mjs';
import {publicDefaultProfile} from './default-profiles.mjs';
import {defaultConversationCustody} from './default-conversation-custody.mjs';

export function createDefaultsMarketplaceFactory(bindings) {
  return async (ctx, {config}) => {
    const resources = requireResourceInputs(bindings), capacity = bindings.defaultsCapacity;
    const owner = config.actors?.user?.subject, admin = config.actors?.admin?.subject;
    required(typeof owner === 'string' && owner.trim() && typeof admin === 'string' && admin.trim()
      && owner !== admin, 'admin_defaults_exact_roles_required');
    required(Number.isSafeInteger(capacity?.cpu_millis) && capacity.cpu_millis > 0
      && Number.isSafeInteger(capacity.memory_mib) && capacity.memory_mib > 0, 'admin_defaults_native_capacity_required');
    const current = await writableSettings(ctx), s = current.settings;
    required(s.inactivity?.auto_pause_enabled === false && s.triggers?.enabled === false
      && s.model_budget?.enabled === false, 'admin_defaults_background_policies_active');
    for (const am of ['a', 'b']) {
      const inventory = (await request(ctx, 'GET', '/api/v1/sandboxes?limit=100', undefined, {actor: 'user', am})).body;
      required(Array.isArray(inventory.items) && inventory.items.length === 0 && inventory.next == null,
        'admin_defaults_empty_owner_required');
    }
    const bounds = (await request(ctx, 'GET', '/api/v1/profile-authoring/bounds', undefined, {actor: 'user'})).body;
    required(capacity.cpu_millis >= bounds.min_cpu_millis && capacity.cpu_millis <= bounds.max_cpu_millis
      && capacity.memory_mib >= bounds.min_memory_mib && capacity.memory_mib <= bounds.max_memory_mib,
    'admin_defaults_capacity_outside_public_bounds');
    const contents = {
      agent: {thinking_effort: 'low', permission_level: 'guarded', skills: [], system_prompt: ''},
      workspace: {location: {type: 'directory', name: 'defaults-' + randomUUID()},
        name: 'Defaults fixture', setup_commands: [], agent_profiles: []},
      sandbox: {cpu_millis: capacity.cpu_millis, memory_mib: capacity.memory_mib,
        nyxid_services: [], agent_provider: 'codex', model_access: 'chronoai_managed',
        max_workspaces: 1, max_agents_per_workspace: 1, workspace_profiles: [],
        software_install_commands: [], environment_variables: []},
    };
    const profiles = {};
    for (const kind of ['agent', 'workspace', 'sandbox']) profiles[kind] = await publicDefaultProfile(ctx, kind, contents[kind], owner);
    const defaults = Object.fromEntries(Object.entries(profiles).map(([kind, value]) => [kind + '_profile', value.reference]));
    const custody = await defaultConversationCustody(ctx, resources, owner, defaults);
    // Provider cleanup releases the global settings lease. Join every owned
    // profile cleanup here first; the earlier defers cover partial setup only.
    let cleanupJob;
    const cleanup = () => cleanupJob ??= (async () => {
      const errors = [];
      for (const entry of [custody, ...Object.values(profiles).reverse()]) {
        try {await entry.cleanup();} catch (error) {errors.push(error);}
      }
      if (errors.length) throw new AggregateError(errors, 'admin_defaults_setup_cleanup_failed');
    })();
    return {defaults, ...custody, cleanup,
      invalidDefaults: [
        {...defaults, agent_profile: {...defaults.agent_profile, revision: 0}},
        {...defaults, workspace_profile: {...defaults.workspace_profile, revision: defaults.workspace_profile.revision + 1}},
      ],
      catalogPath: '/api/v1/marketplace/agent-profiles?' + new URLSearchParams({q: profiles.agent.name, limit: '50'}),
      expectedCatalogIds: [defaults.agent_profile.id],
    };
  };
}
