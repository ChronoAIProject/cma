import {randomUUID} from 'node:crypto';
import {requireResourceInputs} from '../developer-17/setup-resources.mjs';
import {required, check, request, id} from './common.mjs';
import {writableSettings} from './public-setups.mjs';
import {quotaCustody} from './quota-custody.mjs';

// Installed explicitly by the sole parent through adminFactories.user_limit.
// Missing bindings never advertise a seventh built-in public setup.
export function createUserLimitFactory(bindings) {
  return async (ctx, {config}) => {
    const resources = requireResourceInputs(bindings);
    id(resources.sandbox_profile.id);
    const owner = config.actors?.user?.subject, admin = config.actors?.admin?.subject;
    required(typeof owner === 'string' && owner.trim() && typeof admin === 'string'
      && admin.trim() && owner !== admin, 'admin_quota_exact_roles_required');
    const current = await writableSettings(ctx), settings = current.settings;
    required(Number.isSafeInteger(settings.limits?.max_sandboxes_per_user)
      && settings.limits.max_sandboxes_per_user !== 2, 'admin_quota_requires_changed_limit');
    required(settings.inactivity?.auto_pause_enabled === false && settings.triggers?.enabled === false
      && settings.model_budget?.enabled === false, 'admin_quota_background_policies_active');
    // Retired rows are deliberately included. The parent must bind a genuinely
    // empty disposable owner for each repetition; historical data is not hidden.
    for (const am of ['a', 'b']) {
      const response = await request(ctx, 'GET', '/api/v1/sandboxes?limit=100', undefined,
        {actor: 'user', am});
      check(response.status === 200 && Array.isArray(response.body?.items), 'admin_quota_inventory_shape');
      required(response.body.items.length === 0 && response.body.next == null, 'admin_quota_owner_not_empty');
    }
    const intents = Array.from({length: 3}, () => ({method: 'POST', path: '/api/v1/sandboxes',
      actor: 'user', key: randomUUID(), body: {name: 'admin01-quota-' + randomUUID(),
        sandbox_profile: structuredClone(resources.sandbox_profile)}}));
    const custody = await quotaCustody(ctx, resources, owner, intents);
    return {emptyOwner: true, intents: structuredClone(intents), ...custody};
  };
}
