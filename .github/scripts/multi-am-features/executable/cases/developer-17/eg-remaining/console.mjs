import {
  define,
  begin,
  button,
  input,
  visible,
  absent,
  other,
  enc,
  hash,
  same,
  requireInput,
  poll,
  assertNoSecret,
} from './core.mjs';

const recordPath = (record) => '/console/channels/' + enc(record.record_id);
async function navigate(fx, path) {
  requireInput(fx.session.consoleOrigin, 'eg20_console_origin_required');
  await fx.session.page.goto(new URL(path, fx.session.consoleOrigin).href);
}
async function signedIn(ctx, fx) {
  await visible(ctx, button(fx.session.page, 'Sign out'), 'eg20_console_authenticated_controls');
  await absent(
    ctx,
    fx.session.page.getByRole('link', { name: 'Sign in with NyxID', exact: true }),
    'eg20_console_not_login_html',
  );
}
async function login(ctx, fx) {
  const page = fx.session.page,
    identity = fx.lease.console.login;
  await navigate(fx, '/console');
  await page.getByRole('link', { name: 'Sign in with NyxID', exact: true }).click();
  // Credentials/selectors belong to the actual local IdP, never a canned login verdict.
  requireInput(
    identity.origin && identity.usernameLabel && identity.passwordLabel && identity.submitName,
    'eg20_local_idp_form_required',
  );
  ctx.assert(new URL(page.url()).origin === identity.origin, 'eg20_exact_local_idp');
  await input(page, identity.usernameLabel).fill(identity.username);
  await input(page, identity.passwordLabel).fill(identity.password);
  await button(page, identity.submitName).click();
  await page.waitForURL(
    (url) => url.origin === fx.session.consoleOrigin && url.pathname === '/console',
  );
  await signedIn(ctx, fx);
}
async function form(ctx, fx, { path, action, allowed = [303] }) {
  const pending = fx.session.page.waitForResponse(
    (response) =>
      response.request().method() === 'POST' && new URL(response.url()).pathname === path,
  );
  const [response] = await Promise.all([pending, action()]);
  const submitted = Object.fromEntries(new URLSearchParams(response.request().postData()));
  // The route interceptor already retained the form before dispatch, including version.
  ctx.assert(allowed.includes(response.status()), 'eg20_console_form_status');
  await ctx.observe('eg20_console_form', async () => ({
    scope: 'eg',
    path_sha256: hash(path),
    status: response.status(),
    request_id: response.headers()['x-request-id'] ?? null,
    intent_sha256: hash(submitted),
  }));
  return { submitted, status: response.status() };
}
async function loggedOutBoth(ctx, fx) {
  for (const slot of [other(ctx.am), ctx.am]) {
    await fx.session.switchEG(slot);
    await navigate(fx, '/console');
    await visible(
      ctx,
      fx.session.page.getByRole('link', { name: 'Sign in with NyxID', exact: true }),
      'eg20_console_denied_both',
    );
    await absent(
      ctx,
      button(fx.session.page, 'Sign out'),
      'eg20_console_no_authenticated_controls',
    );
  }
}
const authentication = define(70, [
  async (ctx, s) => {
    await begin(ctx, s, 'console-auth');
    await login(ctx, s.fx);
    await s.fx.session.switchEG(other(ctx.am));
    await navigate(s.fx, '/console');
    await signedIn(ctx, s.fx);
  },
  async (ctx, s) => {
    const owner = s.fx.lease.console.allowedOwner;
    requireInput(owner !== 'personal', 'eg20_permitted_org_owner_required');
    await input(s.page, 'Owner').selectOption(owner);
    await form(ctx, s.fx, {
      path: '/console/owner',
      action: () => button(s.page, 'Switch').click(),
    });
    await s.fx.session.switchEG(ctx.am);
    await navigate(s.fx, '/console');
    await signedIn(ctx, s.fx);
    ctx.assert(
      (await input(s.page, 'Owner').inputValue()) === owner,
      'eg20_owner_selection_shared',
    );
    await s.page.locator('form[action="/console/owner"] input[name=csrf]').evaluate((input) => {
      input.value = 'invalid-csrf';
    });
    await form(ctx, s.fx, {
      path: '/console/owner',
      action: () => button(s.page, 'Switch').click(),
      allowed: [403],
    });
    await navigate(s.fx, '/console');
    ctx.assert(
      (await input(s.page, 'Owner').inputValue()) === owner,
      'eg20_csrf_refusal_preserves_owner',
    );
    await form(ctx, s.fx, {
      path: '/console/logout',
      action: () => button(s.page, 'Sign out').click(),
    });
    await loggedOutBoth(ctx, s.fx);
    await login(ctx, s.fx);
    // A short, preconfigured local TTL elapses naturally. No session-store edits.
    const ttl = s.fx.lease.console.expiryWaitMs;
    requireInput(
      Number.isSafeInteger(ttl) && ttl > 0 && ttl <= 30000,
      'eg20_bounded_real_session_ttl_required',
    );
    await new Promise((resolve) => setTimeout(resolve, ttl));
    await loggedOutBoth(ctx, s.fx);
  },
  async (ctx, s) => {
    ctx.assert(
      s.fx.browserIntents.some((intent) => intent.path === '/console/owner') &&
        s.fx.browserIntents.some((intent) => intent.path === '/console/logout'),
      'eg20_console_exact_mutation_custody',
    );
  },
]);
const inspection = define(71, [
  async (ctx, s) => {
    await begin(ctx, s, 'console-inspect');
    await login(ctx, s.fx);
    await navigate(s.fx, recordPath(s.fx.channel));
    await visible(
      ctx,
      s.page.getByRole('heading', { name: s.fx.channel.definition.name, exact: true }),
      'eg20_console_exact_channel',
    );
    for (const name of ['Endpoints', 'Settings', 'Secrets'])
      await visible(
        ctx,
        s.page.getByRole('heading', { name, exact: true }),
        'eg20_console_details',
      );
    await visible(
      ctx,
      s.page.getByText('Only whether each secret is set is shown, never a value.', { exact: true }),
      'eg20_presence_only_secrets',
    );
    await s.page
      .getByRole('navigation', { name: 'Channel', exact: true })
      .getByRole('link', { name: 'Sources', exact: true })
      .click();
    await visible(
      ctx,
      s.page.getByText('custom', { exact: true }).first(),
      'eg20_console_source_identity',
    );
    s.event = await s.fx.emit({ text: 'PRIVATE-EVENT-' + hash(s.fx.channel.record_id) });
    await s.page
      .getByRole('navigation', { name: 'Channel', exact: true })
      .getByRole('link', { name: 'Activity', exact: true })
      .click();
    const link = s.page.locator(`a[href^="${recordPath(s.fx.channel)}/events/"]`);
    await poll(
      ctx,
      async () => {
        await s.page.reload();
        return link.count();
      },
      (count) => count > 0,
      'eg20_console_event_visible',
    );
    await link.first().click();
    await visible(
      ctx,
      s.page.getByRole('heading', { name: 'Routing', exact: true }),
      'eg20_console_event_routing',
    );
    await visible(
      ctx,
      s.page.getByRole('heading', { name: 'Consumers', exact: true }),
      'eg20_console_event_consumers',
    );
    ctx.assert(
      !(await s.page.locator('body').innerText()).includes('PRIVATE-EVENT-'),
      'eg20_no_raw_event_content',
    );
    await assertNoSecret(ctx, await s.page.locator('body').innerText(), s.fx.secrets);
  },
  async (ctx, s) => {
    // This is an actual preexisting local record whose address has not been
    // observed by the Console process. A reset/restart is not performed here.
    const missing = requireInput(
      s.fx.lease.console.missingAddress,
      'eg20_unobserved_console_record_required',
    );
    s.missing = missing;
    const before = await s.fx.read(missing);
    s.before = before;
    await navigate(s.fx, recordPath(missing));
    await visible(ctx, input(s.page, 'Channel id'), 'eg20_missing_address_guarded_form');
    await input(s.page, 'Channel id').fill(missing.channel_id);
    await form(ctx, s.fx, {
      path: recordPath(missing) + '/link',
      action: () => button(s.page, 'Link').click(),
    });
    await visible(
      ctx,
      s.page.getByText(missing.channel_id, { exact: true }).first(),
      'eg20_linked_address_visible',
    );
  },
  async (ctx, s) => {
    const after = await s.fx.read(s.missing, other(ctx.am));
    ctx.assert(
      after.record_id === s.before.record_id &&
        after.channel_id === s.before.channel_id &&
        same(after.definition, s.before.definition),
      'eg20_link_does_not_create_or_mutate_channel',
    );
    ctx.assert(
      !s.fx.browserIntents.some(
        (intent) => intent.method === 'POST' && intent.path === '/api/event-gateway/v1/channels',
      ),
      'eg20_console_link_no_new_channel',
    );
  },
]);
const sources = define(72, [
  async (ctx, s) => {
    await begin(ctx, s, 'console-sources');
    await login(ctx, s.fx);
    s.before = await s.fx.read();
    await navigate(s.fx, recordPath(s.fx.channel) + '?tab=sources');
    const section = s.page.locator('section.attach').filter({
      has: s.page.getByRole('heading', { name: 'Attach a custom source', exact: true }),
    });
    await input(section, 'Authentication').selectOption('nyxid_key');
    s.keyHash = hash('eg20-' + s.fx.channel.record_id);
    await input(section, 'NyxID key hashes').fill(s.keyHash);
    s.attached = await form(ctx, s.fx, {
      path: recordPath(s.fx.channel) + '/sources',
      action: () => button(section, 'Attach').click(),
    });
    ctx.assert(
      Number(s.attached.submitted.expected_version) === s.before.version &&
        s.attached.submitted.csrf,
      'eg20_attach_cas_csrf',
    );
    s.after = await s.fx.read();
    await navigate(s.fx, recordPath(s.fx.channel) + '?tab=sources');
    const staleSection = s.page.locator('section.attach').filter({
      has: s.page.getByRole('heading', { name: 'Attach a custom source', exact: true }),
    });
    await input(staleSection, 'Authentication').selectOption('nyxid_key');
    await input(staleSection, 'NyxID key hashes').fill(hash('stale-' + s.keyHash));
    await staleSection.locator('input[name=expected_version]').evaluate((input, value) => {
      input.value = String(value);
    }, s.before.version);
    await form(ctx, s.fx, {
      path: recordPath(s.fx.channel) + '/sources',
      action: () => button(staleSection, 'Attach').click(),
      allowed: [409],
    });
    ctx.assert(
      same((await s.fx.read()).definition, s.after.definition),
      'eg20_stale_attach_no_mutation',
    );
    s.added = s.after.definition.sources.filter(
      (source) => !s.before.definition.sources.some((old) => same(old, source)),
    );
    ctx.assert(
      s.added.length === 1 && s.added[0].auth.type === 'nyxid_key',
      'eg20_one_supported_source_attached',
    );
  },
  async (ctx, s) => {
    s.detach = recordPath(s.fx.channel) + '/sources/' + 'custom' + '/detach';
    await navigate(s.fx, s.detach);
    for (const name of ['What happens', 'Blocking references', 'Rules that become inactive'])
      await visible(
        ctx,
        s.page.getByRole('heading', { name, exact: true }),
        'eg20_detach_impact_visible',
      );
    const attempted = await form(ctx, s.fx, {
      path: s.detach,
      action: () => button(s.page, 'Detach').click(),
    });
    ctx.assert(
      Number(attempted.submitted.expected_version) === s.after.version && attempted.submitted.csrf,
      'eg20_detach_cas_csrf',
    );
    s.final = await s.fx.read();
  },
  async (ctx, s) => {
    ctx.assert(
      same(s.final.definition.sources, s.before.definition.sources),
      'eg20_only_selected_source_removed',
    );
    ctx.assert(
      s.final.channel_id === s.before.channel_id && s.final.version > s.after.version,
      'eg20_source_mutation_preserves_channel',
    );
    const forbidden = await s.fx.eg({
      path: '/v1/channel-records/' + enc(s.fx.channel.record_id),
      actor: 'outsider',
      allowed: [403, 404],
    });
    ctx.assert(!forbidden.body?.definition, 'eg20_console_channel_owner_guard');
    // A removed source cannot be detached a second time.
    await navigate(s.fx, s.detach); // removed source cannot be detached again
    ctx.assert(!(await button(s.page, 'Detach').count()), 'eg20_removed_source_no_second_detach');
  },
]);
export const cases = [authentication, inspection, sources];
