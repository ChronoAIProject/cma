// Install concrete setup recipes into the existing parent's services. No child
// process, Mongo connection, login grant or application launch occurs here.
import {createHash, randomUUID} from 'node:crypto';
import {Blocked} from './runner.mjs';

const check = (value, code) => {if (!value) throw Error(code);};
const digest = value => createHash('sha256').update(JSON.stringify(value)).digest('hex');

export function disposableCustody(config, record) {
  check(config.scope === 'LOCAL_DISPOSABLE_TWO_AM' && config.database_disposable === true
    && typeof config.run_id === 'string' && config.run_id.length > 0, 'disposable_parent_binding_required');
  return {scope: config.scope, database_disposable: true, async reserve({purpose, caseId}) {
    const reservation = randomUUID(); let released = false;
    const retain = async (kind, value) => {
      check(!released, 'disposable_custody_released');
      await record({kind, campaign_id: config.run_id, reservation, purpose, case_id: caseId, value});
    };
    await retain('disposable_reservation', null);
    return {parent_cleanup_barrier: true,
      async retainIntent(intent) { await retain('disposable_intent', intent); },
      async retainDocument(id) {
        check(typeof id === 'string' && id.length > 0, 'document_custody_identity_required');
        await retain('disposable_document', {id});
      },
      async release() {
        if (!released) {
          await retain('disposable_reservation_closed', {final_cleanup: 'PENDING_PARENT_DATABASE_DISPOSAL'});
          released = true;
        }
        return {registered: true, parent_cleanup_barrier: true,
          reservation_sha256: digest(reservation), final_cleanup: 'PENDING_PARENT_DATABASE_DISPOSAL'};
      },
    };
  }};
}

export async function bindParentFixtures(config, context, transport) {
  const {adapter, services} = context;
  const supplied = services.executable ?? {};
  const shared = {...supplied, fixtures: {...supplied.fixtures}, primitives: {...supplied.primitives}};
  shared.primitives.disposableDatabase ??= disposableCustody(config, event => services.record(event));
  // Subjects must be provided alongside exact parent actor bindings. A guessed
  // or privileged fallback identity is never substituted for another account.
  shared.primitives.identities ??= Object.fromEntries([
    ...(config.owner_id ? [['user', {actor: 'user', subject: config.owner_id}]] : []),
    ...Object.entries(config.actors ?? {}).filter(([actor, value]) => transport.hasActor(actor) && value?.subject)
      .map(([actor, value]) => [actor, {actor, subject: value.subject}]),
  ]);
  if (transport.hasActor('admin') && typeof services.withExclusiveSettings === 'function') {
    const {createAdminFixtures} = await import('./cases/developer-01/fixture-provider.mjs');
    const provider = createAdminFixtures({config, adapter,
      withExclusiveSettings: work => services.withExclusiveSettings(work),
      factories: supplied.adminFactories ?? {}});
    shared.fixtures['admin-fixtures'] ??= provider;
    Object.assign(shared.fixtures, provider.capabilities);
  }
  if (supplied.userSetup?.config && typeof supplied.userSetup.finality === 'function') {
    const {createUserFixtureFactories} = await import('./cases/developer-06/setups.mjs');
    const recipes = createUserFixtureFactories({...supplied.userSetup,
      request: request => transport.request(request)});
    const parent = shared.fixtures['user-fixtures'];
    shared.fixtures['user-fixtures'] = {...parent, async take(name, ctx) {
      if (recipes[name]) return recipes[name](ctx);
      if (typeof parent?.take === 'function') return parent.take(name, ctx);
      throw new Blocked('user_setup_unavailable');
    }};
  }
  return shared;
}
