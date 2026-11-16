import {
  define,
  begin,
  button,
  input,
  choose,
  openChannel,
  selectNode,
  uiMutation,
  visible,
  absent,
  other,
  enc,
  same,
  hash,
  unique,
  requireInput,
  assertIdentity,
  assertNoSecret,
  listField,
} from './core.mjs';
import { channelDraft, profileFields } from './forms.mjs';
const path = (fx) => `/v1/channels/${enc(fx.channel.channel_id)}`;
const ui = (path) => '/api/event-gateway' + path;
async function create(ctx, s) {
  s.created = await uiMutation(ctx, s.fx, {
    method: 'POST',
    path: ui('/v1/channels'),
    action: () => button(s.page, 'Create channel').click(),
    allowed: [201],
    kind: 'channel',
  });
  s.fx.channel = s.created.body;
  ctx.assert(
    s.created.submitted.record_id === s.created.body.record_id &&
      s.created.body.definition.name === s.fx.channelName,
    'eg20_durable_creation_identity',
  );
}
const createCase = define(20, [
  async (ctx, s) => {
    await begin(ctx, s, 'create');
    await channelDraft(ctx, s.fx);
    ctx.assert(await button(s.page, 'Create channel').isEnabled(), 'eg20_reviewed_channel_ready');
  },
  async (ctx, s) => {
    await create(ctx, s);
    await s.fx.session.switchEG(other(ctx.am));
    await s.page.reload();
    await openChannel(ctx, s.fx);
    s.read = await s.fx.read();
    assertIdentity(ctx, s.read, s.created.body);
    ctx.assert(
      same(s.read.definition.sources, s.created.body.definition.sources),
      'eg20_peer_source_readback',
    );
  },
  async (ctx, s) => {
    ctx.assert(
      s.fx.browserIntents.filter((r) => r.method === 'POST' && r.path === ui('/v1/channels'))
        .length === 1,
      'eg20_single_create_request',
    );
    assertNoSecret(ctx, s.read, s.fx.secrets);
    ctx.assert(
      s.read.version >= s.created.body.version && s.read.definition.creation.ready === true,
      'eg20_completed_creation_revision',
    );
  },
]);
const recover = define(21, [
  async (ctx, s) => {
    await begin(ctx, s, 'recover');
    await channelDraft(ctx, s.fx);
    s.cut = await s.fx.fault('dropAcceptedResponse', {
      method: 'POST',
      path: ui('/v1/channels'),
      count: 1,
    });
    requireInput(typeof s.cut.join === 'function', 'eg20_fault_acceptance_journal_required');
    await button(s.page, 'Create channel').click();
    s.accepted = await s.cut.join();
    ctx.assert(
      s.accepted.accepted === true &&
        s.accepted.method === 'POST' &&
        s.accepted.request.body.record_id,
      'eg20_unknown_create_retains_original_intent',
    );
    await s.fx.capture({
      status: s.accepted.status,
      body: s.accepted.body,
      submitted: s.accepted.request.body,
    });
    await visible(ctx, button(s.page, 'Check original channel'), 'eg20_recovery_action_visible');
  },
  async (ctx, s) => {
    await s.cut.restore();
    await s.fx.session.switchEG(other(ctx.am));
    await button(s.page, 'Check original channel').click();
    s.read = (
      await s.fx.eg({
        path: `/v1/channel-records/${enc(s.accepted.request.body.record_id)}`,
        replica: other(ctx.am),
      })
    ).body;
    ctx.assert(
      s.read.record_id === s.accepted.request.body.record_id &&
        s.read.channel_id === s.accepted.body.channel_id,
      'eg20_recovery_exact_record',
    );
    ctx.assert(s.read.version === s.accepted.body.version, 'eg20_recovery_exact_settled_revision');
  },
  async (ctx, s) => {
    ctx.assert(
      s.fx.browserIntents.filter((r) => r.path === ui('/v1/channels') && r.method === 'POST')
        .length === 1,
      'eg20_no_replay_after_unknown',
    );
    assertNoSecret(ctx, s.read, s.fx.secrets);
    await ctx.observe('eg20_original_intent', async () => ({
      record_sha256: hash(s.read.record_id),
      request_sha256: hash(s.accepted.request.body),
      version: s.read.version,
    }));
  },
]);
const capabilities = define(19, [
  async (ctx, s) => {
    await begin(ctx, s, 'capabilities');
    s.cap = (await s.fx.eg({ path: '/v1/channel-capabilities' })).body;
    ctx.assert(
      Array.isArray(s.cap.relay_authoring?.platforms) && s.cap.relay_authoring,
      'eg20_actual_capability_discovery',
    );
    await channelDraft(ctx, s.fx);
    await choose(s.page, 'Producer 1 connection type', 'NyxID messaging connection');
    ctx.assert(
      (await input(s.page, 'Channel name').inputValue()) === s.fx.channelName,
      'eg20_capability_keeps_draft',
    );
  },
  async (ctx, s) => {
    await button(s.page, 'Producer 1 connection type').click();
    const authoring = s.cap.relay_authoring;
    const native = [
      [
        authoring.native_discord_writes_enabled === true &&
          authoring.native_discord_auto_activation_enabled === true &&
          authoring.multi_consumer_writes_enabled === true,
        'Direct Discord bot',
      ],
      [
        authoring.native_whatsapp_writes_enabled === true &&
          authoring.native_whatsapp_auto_activation_enabled === true,
        'Direct WhatsApp Business',
      ],
      [
        authoring.native_lark_writes_enabled === true &&
          authoring.native_lark_intake_enabled === true,
        'Direct Lark / Feishu app',
      ],
    ];
    ctx.assert(
      native.some(([enabled]) => !enabled),
      'eg20_disabled_authoring_variant_present',
    );
    for (const [enabled, label] of native)
      if (!enabled)
        await absent(
          ctx,
          s.page.getByRole('menuitemradio', { name: label, exact: true }),
          'eg20_disabled_new_source_absent',
        );
    await s.page.keyboard.press('Escape');
    const saved = requireInput(
      s.fx.authority.savedDisabledChannel,
      'eg20_saved_disabled_source_fixture_required',
    );
    s.saved = (await s.fx.eg({ path: `/v1/channel-records/${enc(saved.recordId)}` })).body;
    await openChannel(ctx, s.fx, s.saved);
    await selectNode(ctx, s.fx, saved.nodeLabel);
    await visible(
      ctx,
      s.page.getByText(saved.disabledNotice, { exact: false }),
      'eg20_saved_disabled_setting_retained',
    );
  },
  async (ctx, s) => {
    const current = await s.fx.read(s.saved);
    ctx.assert(same(current, s.saved), 'eg20_capability_never_downgrades_saved');
    ctx.assert(s.fx.browserIntents.length === 0, 'eg20_discovery_has_no_mutation');
    ctx.assert(
      !s.fx.responses.some((r) => r.path?.endsWith('/test')),
      'eg20_discovery_not_connectivity_test',
    );
  },
]);
const sources = define(23, [
  async (ctx, s) => {
    await begin(ctx, s, 'sources');
    s.before = await s.fx.read();
    await openChannel(ctx, s.fx);
    await selectNode(ctx, s.fx, /^Producer: Custom events/);
    await input(s.page, 'Prompt template').fill('Coverage event: {{event}}');
    s.saved = (
      await uiMutation(ctx, s.fx, {
        method: 'PUT',
        path: ui(path(s.fx)),
        action: () => button(s.page, 'Save producer').click(),
      })
    ).body;
    ctx.assert(
      s.saved.version > s.before.version &&
        s.saved.definition.sources[0].prompt_template === 'Coverage event: {{event}}',
      'eg20_exact_producer_edit',
    );
    ctx.assert(
      s.before.definition.secrets && same(s.saved.definition.secrets, s.before.definition.secrets),
      'eg20_omitted_credentials_preserved',
    );
  },
  async (ctx, s) => {
    requireInput(s.fx.authority.additionalSource, 'eg20_second_supported_source_required');
    await openChannel(ctx, s.fx);
    await button(s.page, 'Add producer').click();
    await choose(s.page, 'Producer 2 connection type', 'NyxID messaging connection');
    const source = s.fx.authority.additionalSource;
    await choose(s.page, 'Messaging platform', source.platformLabel);
    await input(s.page, 'NyxID issuer').fill(source.issuer);
    await input(s.page, 'Signing key ID').fill(source.key_id);
    await listField(s.page, 'Route IDs', source.route_ids);
    s.added = (
      await uiMutation(ctx, s.fx, {
        method: 'PUT',
        path: ui(path(s.fx)),
        action: () => button(s.page, 'Save producer').click(),
      })
    ).body;
    ctx.assert(
      s.added.definition.sources.length === s.before.definition.sources.length + 1,
      'eg20_added_one_producer',
    );
    ctx.assert(
      (await button(s.page, 'Remove producer').count()) === 1,
      'eg20_product_missing_producer_removal_control',
    );
    await button(s.page, 'Remove producer').click();
    await visible(ctx, s.page.getByText(/routing|reference/i), 'eg20_remove_impact_visible');
    s.removed = (
      await uiMutation(ctx, s.fx, {
        method: 'PUT',
        path: ui(path(s.fx)),
        action: () => button(s.page, 'Confirm remove').click(),
      })
    ).body;
    ctx.assert(
      same(s.removed.definition.sources, s.saved.definition.sources),
      'eg20_only_added_source_removed',
    );
  },
  async (ctx, s) => {
    const current = await s.fx.read();
    assertIdentity(ctx, current, s.removed);
    ctx.assert(
      s.before.definition.secrets && same(current.definition.secrets, s.before.definition.secrets),
      'eg20_secret_metadata_preserved',
    );
    const observed = await s.fx.platform?.journal({ recordId: current.record_id });
    if (observed)
      ctx.assert(
        !observed.some((r) => r.kind === 'credential_replace'),
        'eg20_no_credential_rotation',
      );
    assertNoSecret(ctx, current, s.fx.secrets);
  },
]);
const profile = define(25, [
  async (ctx, s) => {
    await begin(ctx, s, 'profile');
    s.before = await s.fx.read();
    await openChannel(ctx, s.fx);
    await selectNode(ctx, s.fx, /^Consumer: Default consumer/);
    await visible(
      ctx,
      s.page.getByText(`Provider: ${s.fx.authority.provider.slug}`, { exact: true }),
      'eg20_provider_destination_visible',
    );
    ctx.assert(
      s.before.provider.slug === s.fx.authority.provider.slug,
      'eg20_allowlisted_provider_identity',
    );
  },
  async (ctx, s) => {
    s.profile = structuredClone(s.fx.authority.profile);
    s.profile.context.system_prompt = unique('Coverage prompt');
    await profileFields(s.fx, { profile: s.profile });
    s.saved = await uiMutation(ctx, s.fx, {
      method: 'PUT',
      path: ui(path(s.fx)),
      action: () => button(s.page, 'Save agent settings').click(),
    });
    ctx.assert(same(s.saved.body.definition.profile, s.profile), 'eg20_exact_profile_saved');
    ctx.assert(
      !s.saved.submitted.credentials ||
        Object.values(s.saved.submitted.credentials).every((v) => v === ''),
      'eg20_profile_keeps_stored_credentials',
    );
  },
  async (ctx, s) => {
    const peer = await s.fx.read();
    assertIdentity(ctx, peer, s.saved.body);
    ctx.assert(
      same(peer.definition.profile, s.profile) && same(peer.provider, s.saved.body.provider),
      'eg20_profile_peer_identity',
    );
    assertNoSecret(ctx, peer, s.fx.secrets);
  },
]);

export const cases = [capabilities, createCase, recover, sources, profile];
