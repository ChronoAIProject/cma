import {isDeepStrictEqual as equal} from 'node:util';
import {Blocked} from '../../runner.mjs';

export {equal};
export const P = '/api/v1/admin';
export const other = am => am === 'a' ? 'b' : 'a';
export const check = (value, code) => { if (!value) throw Error(code); };
export const required = (value, code) => { if (!value) throw new Blocked(code); };
export const id = value => {
  required(typeof value === 'string' && /^[-a-zA-Z0-9_]{1,160}$/.test(value), 'admin_fixture_id');
  return encodeURIComponent(value);
};
export const normalizedSettings = value => {
  const copy = structuredClone(value);
  if (copy.inactivity) delete copy.inactivity.grace_started_at;
  return copy;
};
export const sameSettings = (a, b) => equal(normalizedSettings(a), normalizedSettings(b));
export const request = async (ctx, method, path, body, options = {}) =>
  ctx.request({actor: 'admin', am: ctx.am, method, path, body, ...options});
export const read = async (ctx, path, am = ctx.am) =>
  (await request(ctx, 'GET', path, undefined, {am})).body;

export async function setup(ctx, name) {
  const provider = ctx.fixture('admin-fixtures');
  required(typeof provider.take === 'function', 'admin_fixture_provider_missing');
  const fixture = await provider.take(name, ctx);
  required(typeof fixture?.cleanup === 'function', 'admin_fixture_cleanup_missing');
  ctx.defer('admin_fixture_cleanup', () => fixture.cleanup());
  required(fixture.scope === 'LOCAL_DISPOSABLE_TWO_AM' && fixture.createdByRun === true,
    'admin_fixture_not_local_disposable');
  return fixture;
}

export async function browser(ctx, actor = 'admin', am = ctx.am, agentId) {
  const adapter = ctx.fixture('browser');
  const session = await adapter.open({actor, am, agentId});
  required(typeof session?.close === 'function', 'admin_browser_cleanup_missing');
  ctx.defer('admin_browser_close', () => session.close());
  required(session.page && session.origin && typeof session.switchAM === 'function',
    'admin_browser_contract');
  return session;
}

export async function evidence(ctx, session) {
  const value = await ctx.observe('admin_browser_evidence', () => ctx.fixture('browser').evidence(session.page));
  ctx.assert(Array.isArray(value.requests) && value.requests.length > 0
    && value.requests.every(r => r.request_id && ['a', 'b'].includes(r.serving_am)),
  'admin_browser_request_attribution');
  return value;
}

export async function roleRequest(ctx, fixture, role, method, path, body, options = {}) {
  required(typeof fixture.withRole === 'function', 'admin_isolated_role_binding_missing');
  let calls = 0;
  const response = await fixture.withRole(role, alias => {
    check(++calls === 1, 'admin_role_callback_replayed');
    return request(ctx, method, path, body, {...options, actor: alias});
  });
  check(calls === 1, 'admin_role_callback_not_called');
  return response;
}

export async function settingsSession(ctx, fixture) {
  required(fixture.exclusiveSettings === true, 'admin_settings_lease_missing');
  const original = await read(ctx, P + '/settings');
  check(Number.isSafeInteger(original.revision) && original.settings, 'admin_settings_shape');
  const owned = [structuredClone(original.settings)];
  let restored = false;
  const restore = async () => {
    if (restored) return;
    const current = await read(ctx, P + '/settings', other(ctx.am));
    if (!sameSettings(current.settings, original.settings)) {
      check(owned.some(value => sameSettings(value, current.settings)), 'admin_restore_foreign_settings');
      await request(ctx, 'PUT', P + '/settings', {
        expected_revision: current.revision, settings: original.settings,
      }, {am: other(ctx.am)});
    }
    const final = await read(ctx, P + '/settings');
    check(sameSettings(final.settings, original.settings), 'admin_restore_failed');
    restored = true;
  };
  ctx.defer('admin_settings_restore', restore);
  return {original, own: value => owned.push(structuredClone(value)), restore,
    async patch(section, value) {
      const before = await read(ctx, P + '/settings');
      const desired = {...before.settings, [section]: value};
      check(!sameSettings(desired, before.settings), 'admin_settings_no_change');
      // Retain intent before dispatch: an unknown acknowledgement still requires readback.
      owned.push(structuredClone(desired));
      const response = await request(ctx, 'PATCH', P + '/settings', {
        expected_revision: before.revision, [section]: value,
      });
      check(response.body.revision > before.revision, 'admin_settings_revision_not_advanced');
      return response.body;
    }};
}

export async function navigateSettings(session) {
  await session.page.goto(new URL('/operations?tab=settings', session.origin).href);
  await session.page.getByRole('region', {name: 'Platform settings', exact: true}).waitFor();
  await session.page.getByLabel('Agent log retention (days)', {exact: true}).waitFor();
}

export async function saveSettings(session) {
  await session.page.getByRole('button', {name: 'Save settings', exact: true}).click();
  await session.page.getByText(/^Settings saved at revision /).waitFor();
}

export async function invalidSettings(ctx, body, allowed = [400, 422]) {
  const before = await read(ctx, P + '/settings');
  await request(ctx, 'PATCH', P + '/settings', {expected_revision: before.revision, ...body}, {allowed});
  const after = await read(ctx, P + '/settings', other(ctx.am));
  ctx.assert(equal(before, after), 'admin_invalid_settings_changed_state');
}

export function define(idNumber, scope, steps, run, fixtures = ['admin-fixtures']) {
  return {id: `C275:ADM-${String(idNumber).padStart(3, '0')}`, scope,
    steps: [...Array.from({length: steps}, (_, i) => 'step_' + (i + 1)), 'expected_result'],
    fixtures, run};
}
