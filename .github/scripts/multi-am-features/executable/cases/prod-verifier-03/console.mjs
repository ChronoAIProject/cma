import {define, acquire, invoke, visible, egRequest, peer, segment, same, digest} from './helpers.mjs';
import {Blocked} from '../../runner.mjs';

const consoleInvoke = (ctx, session, method, args = {}) =>
  invoke(ctx.fixture('eg_console'), method, {session, page: session.page, ...args});

async function openConsole(ctx, lease, options = {}) {
  const adapter = ctx.fixture('eg_console');
  const session = await invoke(adapter, 'open', {actor: 'owner', replica: ctx.am, lease, ...options});
  if (typeof session?.close !== 'function') throw Error('console_cleanup_required');
  ctx.defer('close_console', () => session.close());
  ctx.defer('retain_console_evidence', async () => {
    const evidence = await invoke(adapter, 'evidence', session.page);
    if (evidence.schema !== 'multi-eg-browser-evidence-v1' || evidence.authentication !== 'browser_session'
      || evidence.real_ui !== true || evidence.secrets_redacted !== true
      || !evidence.requests?.length || evidence.requests.some(r => !r.request_id
        || !lease.replicas.some(eg => eg.slot === r.replica && eg.instance === r.instance)))
      throw new Blocked('console_real_eg_session_evidence_required');
    await ctx.observe('console_evidence', async () => evidence);
  });
  ctx.assert(session.authentication === 'browser_session' && typeof session.page?.getByRole === 'function',
    'console_real_page_session_required');
  const go = path => session.page.goto(new URL(path, session.origin).href);
  return {...session, go};
}
const path = lease => `/console/channels/${segment(lease.recordId)}`;
const tab = (page, name) => page.getByRole('navigation', {name: 'Channel', exact: true})
  .getByRole('link', {name, exact: true}).click();
async function signedIn(ctx, s, lease) {
  await visible(ctx, s.page.getByRole('button', {name: 'Sign out', exact: true}), 'console_authenticated_controls');
  ctx.assert(await s.page.getByRole('link', {name: 'Sign in with NyxID', exact: true}).count() === 0,
    'console_not_signin_html_success');
  await visible(ctx, s.page.getByRole('link', {name: lease.channelName, exact: true}), 'console_owned_inventory');
}

export const consoleCases = [
  define(70, 'browser', ['user-fixtures', 'eg_console'], async ctx => {
    let lease, s;
    await ctx.step('step_1', async () => {
      lease = await acquire(ctx, 'console_session'); s = await openConsole(ctx, lease, {authenticated: false});
      await s.go('/console');
      const link = s.page.getByRole('link', {name: 'Sign in with NyxID', exact: true});
      await visible(ctx, link, 'console_explicit_login_link');
      ctx.assert(new URL(await link.getAttribute('href'), s.origin).pathname === '/console/login', 'console_local_login_entry');
      await link.click(); await consoleInvoke(ctx, s, 'completeLocalLogin');
      await signedIn(ctx, s, lease);
      await s.switchEG(peer(ctx.am)); await s.go('/console'); await signedIn(ctx, s, lease);
    });
    await ctx.step('step_2', async () => {
      await s.page.getByLabel('Owner', {exact: true}).selectOption(lease.expected.organizationId);
      await s.page.getByRole('button', {name: 'Switch', exact: true}).click();
      await visible(ctx, s.page.getByText(`organization ${lease.expected.organizationName}`, {exact: false}),
        'console_permitted_organization_scope');
      ctx.assert(await s.page.getByRole('link', {name: lease.expected.personalChannelName, exact: true}).count() === 0,
        'console_personal_data_not_in_organization');
      const foreign = await consoleInvoke(ctx, s, 'ownerAttempt', {owner: lease.expected.unrelatedOrganizationId});
      ctx.assert(foreign.authorized === false && foreign.channelCount === 0, 'console_arbitrary_owner_denied');
      const form = s.page.locator('form[action="/console/owner"]');
      await form.locator('input[name="csrf"]').evaluate(input => {input.value = '';});
      const denied = s.page.waitForResponse(r => new URL(r.url()).pathname === '/console/owner' && r.request().method() === 'POST');
      await form.getByRole('button', {name: 'Switch', exact: true}).click();
      ctx.assert((await denied).status() === 403, 'console_csrf_refused');
      await s.go('/console');
      await s.page.getByRole('button', {name: 'Sign out', exact: true}).click();
      await visible(ctx, s.page.getByRole('heading', {name: 'Signed out', exact: true}), 'console_signed_out_page');
      for (const replica of [ctx.am, peer(ctx.am)]) {
        await s.switchEG(replica); await s.go('/console');
        await visible(ctx, s.page.getByRole('link', {name: 'Sign in with NyxID', exact: true}), 'console_logout_shared_across_egs');
        ctx.assert(await s.page.getByRole('button', {name: 'Sign out', exact: true}).count() === 0, 'console_logout_no_authenticated_controls');
      }
      const expired = await openConsole(ctx, lease, {authenticated: true});
      await consoleInvoke(ctx, expired, 'expireSession');
      for (const replica of [ctx.am, peer(ctx.am)]) {
        await expired.switchEG(replica); await expired.go('/console');
        ctx.assert(await expired.page.getByRole('button', {name: 'Sign out', exact: true}).count() === 0,
          'console_expiry_denies_both_replicas');
        ctx.assert(await expired.page.getByRole('heading', {name: /Sign in to the console|Signed out/}).count() === 1,
          'console_expiry_visible');
      }
    });
    await ctx.step('expected_result', async () => {
      const auth = await consoleInvoke(ctx, s, 'sessionMetadata');
      ctx.assert(auth.revoked === true && auth.cookieHttpOnly === true && auth.cookieSameSite === 'Lax',
        'console_cookie_revoked_and_protected');
      await ctx.observe('console_session_receipt', async () => ({revoked: auth.revoked, http_only: auth.cookieHttpOnly}));
    });
  }),
  define(71, 'browser', ['user-fixtures', 'eg_console'], async ctx => {
    let lease, s, before;
    await ctx.step('step_1', async () => {
      lease = await acquire(ctx, 'console_inspection'); s = await openConsole(ctx, lease, {authenticated: true});
      before = await egRequest(ctx, lease, {path: `/v1/channels/${segment(lease.channelId)}`});
      await s.go(path(lease));
      await visible(ctx, s.page.getByRole('heading', {name: lease.channelName, exact: true}), 'console_channel_record');
      for (const name of ['Endpoints', 'Settings', 'Secrets']) await visible(ctx,
        s.page.getByRole('heading', {name, exact: true}), 'console_settings_section');
      await visible(ctx, s.page.getByText('Only whether each secret is set is shown, never a value.', {exact: true}),
        'console_secret_presence_only');
      await tab(s.page, 'Sources');
      ctx.assert((await s.page.locator('main').innerText()).includes(lease.expected.sourceId), 'console_source_identity');
      await tab(s.page, 'Activity');
      await s.page.locator(`a[href="${path(lease)}/events/${segment(lease.expected.eventKey)}"]`).click();
      for (const marker of lease.expected.eventMarkers) await visible(ctx,
        s.page.getByText(marker, {exact: false}), 'console_bounded_event_detail');
      ctx.assert(await s.page.getByText(lease.expected.foreignTranscriptMarker, {exact: false}).count() === 0,
        'console_no_unrelated_transcript');
    });
    await ctx.step('step_2', async () => {
      await s.go(`/console/channels/${segment(lease.expected.missingRecordId)}`);
      await visible(ctx, s.page.getByLabel('Channel id', {exact: true}), 'console_missing_address_form');
      await s.page.getByLabel('Channel id', {exact: true}).fill(lease.expected.missingChannelId);
      await s.page.getByRole('button', {name: 'Link', exact: true}).click();
      await visible(ctx, s.page.getByRole('heading', {name: 'Endpoints', exact: true}), 'console_link_recovers_endpoints');
      ctx.assert(await s.page.getByLabel('Channel id', {exact: true}).count() === 0, 'console_address_no_longer_missing');
      await s.switchEG(peer(ctx.am)); await s.go(`/console/channels/${segment(lease.expected.missingRecordId)}`);
      const linked = await consoleInvoke(ctx, s, 'linkedAddress', {recordId: lease.expected.missingRecordId});
      ctx.assert(linked.channelId === lease.expected.missingChannelId && linked.createdChannels === 0,
        'console_link_exact_existing_address');
    });
    await ctx.step('expected_result', async () => {
      const after = await egRequest(ctx, lease, {path: `/v1/channels/${segment(lease.channelId)}`, replica: peer(ctx.am)});
      ctx.assert(same(after, before), 'console_inspection_does_not_change_channel');
      const scan = await invoke(lease, 'scanBrowserForCredentialValues', {page: s.page});
      ctx.assert(scan.matches === 0 && scan.scannedText && scan.scannedInputs, 'console_no_secret_disclosure');
      await ctx.observe('console_inspection', async () => ({record_sha256: digest(after.record_id), version: after.version}));
    });
  }),
  define(72, 'browser', ['user-fixtures', 'eg_console'], async ctx => {
    let lease, s, before, added, source;
    await ctx.step('step_1', async () => {
      lease = await acquire(ctx, 'console_source_lifecycle'); s = await openConsole(ctx, lease, {authenticated: true});
      before = await egRequest(ctx, lease, {path: `/v1/channels/${segment(lease.channelId)}`});
      await s.go(path(lease) + '?tab=sources');
      const section = s.page.locator('section.attach').filter({has: s.page.getByRole('heading', {name: 'Attach a custom source', exact: true})});
      await section.getByLabel('Authentication', {exact: true}).selectOption('nyxid_key');
      await section.getByLabel('NyxID key hashes', {exact: true}).fill(lease.expected.syntheticKeyDigest);
      ctx.assert(Number(await section.locator('input[name="expected_version"]').inputValue()) === before.version,
        'console_attach_expected_version');
      await section.getByRole('button', {name: 'Attach', exact: true}).click();
      added = await egRequest(ctx, lease, {path: `/v1/channels/${segment(lease.channelId)}`, replica: peer(ctx.am)});
      source = added.definition.sources.find(item => item.type === 'custom');
      ctx.assert(!before.definition.sources.some(item => item.type === 'custom')
        && added.definition.sources.length === before.definition.sources.length + 1, 'console_new_custom_source_slot');
      ctx.assert(source?.type === 'custom' && added.version > before.version
        && same(source.auth.key_hashes, [lease.expected.syntheticKeyDigest]), 'console_attached_exact_source');
    });
    await ctx.step('step_2', async () => {
      const detachPath = `${path(lease)}/sources/custom/detach`;
      await s.go(detachPath);
      for (const name of ['What happens', 'Blocking references', 'Rules that become inactive'])
        await visible(ctx, s.page.getByRole('heading', {name, exact: true}), 'console_detach_impact_preview');
      ctx.assert((await egRequest(ctx, lease, {path: `/v1/channels/${segment(lease.channelId)}`})).version === added.version,
        'console_preview_no_mutation');
      const denied = await consoleInvoke(ctx, s, 'submitGuardedFormVariant', {path: detachPath, variant: 'stale_revision'});
      ctx.assert(denied.status === 409 && denied.version === added.version, 'console_detach_stale_version_refused');
      const csrf = await consoleInvoke(ctx, s, 'submitGuardedFormVariant', {path: detachPath, variant: 'invalid_csrf'});
      ctx.assert(csrf.status === 403 && csrf.version === added.version, 'console_detach_invalid_csrf_refused');
      await s.page.getByRole('button', {name: 'Detach', exact: true}).click();
      const remaining = await egRequest(ctx, lease, {path: `/v1/channels/${segment(lease.channelId)}`, replica: peer(ctx.am)});
      ctx.assert(remaining.version > added.version && same(remaining.definition.sources, before.definition.sources),
        'console_only_selected_source_detached');
      await s.go(`${path(lease)}/sources/${segment(lease.expected.blockingSourceId)}/detach`);
      await visible(ctx, s.page.getByText(/The detach is refused with 409 until the routing policy names another destination/),
        'console_blocking_route_visible');
      const blocked = s.page.waitForResponse(r => r.request().method() === 'POST' && new URL(r.url()).pathname.endsWith('/detach'));
      await s.page.getByRole('button', {name: 'Detach', exact: true}).click();
      ctx.assert((await blocked).status() === 409, 'console_blocking_reference_prevents_detach');
    });
    await ctx.step('expected_result', async () => {
      const foreign = await consoleInvoke(ctx, s, 'submitGuardedFormVariant', {path: path(lease) + '/sources', variant: 'wrong_owner'});
      ctx.assert(foreign.status === 403 || foreign.status === 404, 'console_source_owner_enforced');
      const final = await egRequest(ctx, lease, {path: `/v1/channels/${segment(lease.channelId)}`});
      ctx.assert(same(final.definition.sources, before.definition.sources), 'console_source_finality');
      await ctx.observe('console_source_revision', async () => ({before: before.version, after: final.version}));
    });
  }),
];
