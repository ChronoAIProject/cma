import {Blocked} from './runner.mjs';

// Providers belong to one author directory. Shared primitives are explicit
// parent capabilities; a missing provider never falls back to a case oracle.
export function fixturesForCase(registry, definition, shared = {}) {
  const fixtures = {...shared.fixtures};
  const providers = registry.setups?.get(definition.owner) ?? {};
  const namespace = definition.id.startsWith('API:') ? 'api-fixtures'
    : definition.id.startsWith('C275:ADM-') ? 'admin-fixtures' : 'user-fixtures';
  if (Object.keys(providers).length) {
    const parent = fixtures[namespace];
    fixtures[namespace] = {...parent, async take(name, ctx) {
      if (typeof providers[name] !== 'function' && typeof parent?.take !== 'function') {
        throw new Blocked('setup_unavailable_' + namespace.replaceAll('-', '_'));
      }
      const value = providers[name] ? await providers[name](ctx, shared.primitives ?? {}) : await parent.take(name, ctx);
      if (!value || typeof value.cleanup !== 'function') throw new Blocked('setup_cleanup_unavailable');
      return value;
    }};
  }
  return fixtures;
}
