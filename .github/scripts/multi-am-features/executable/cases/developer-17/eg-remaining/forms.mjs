import { button, input, choose, toggle, listField, visible, requireInput } from './core.mjs';

export async function channelDraft(ctx, fx, { connectionRef } = {}) {
  const page = fx.session.page;
  await button(page, 'New channel').click();
  await input(page, 'Channel name').fill(fx.channelName);
  if (connectionRef) {
    await toggle(page, 'Use an existing CMA-managed Telegram connection', true);
    const choice = requireInput(
      fx.authority.connectionLabel,
      'eg20_owned_connection_label_required',
    );
    let pages = 0;
    while (await button(page, 'Load more connections').count()) {
      requireInput(pages++ < 20, 'eg20_connection_ui_page_bound');
      const response = page.waitForResponse(
        (r) =>
          r.request().method() === 'GET' &&
          new URL(r.url()).pathname === '/api/event-gateway/v1/channel-connections',
      );
      await button(page, 'Load more connections').click();
      await response;
    }
    requireInput(pages > 0, 'eg20_connection_ui_pagination_required');
    await choose(page, 'Saved Telegram connection', choice);
    await choose(page, 'Connection admission policy', 'Allowed senders');
    await listField(page, 'Allowed sender IDs', fx.authority.connectionSenderIds);
  } else {
    await choose(page, 'Producer 1 connection type', 'Custom event');
    await choose(page, 'Authentication method', 'HMAC signature');
    await input(page, 'HMAC key ID').fill('eg20-hmac');
    await toggle(page, 'Replace HMAC credentials', true);
    await button(page, 'Add HMAC credential').click();
    await input(page, 'HMAC 1 key ID').fill('eg20-hmac');
    await input(page, 'HMAC 1 secret').fill(fx.hmac);
  }
  await input(page, 'Provider').fill(fx.authority.provider.slug);
  const target = fx.authority.provider.target;
  if (target?.type === 'existing_agent') {
    await choose(page, 'Agent target', 'Use an existing agent');
    await input(page, 'Agent ID').fill(target.agent_id);
  }
  await profileFields(fx, { creation: true });
  await input(page, 'Provider agent key').fill(fx.authority.agentKey);
  await input(page, 'Acknowledgement delay (ms)').fill('2000');
  await toggle(page, 'Send final answers automatically', true);
  ctx.assert(
    (await input(page, 'Channel name').inputValue()) === fx.channelName,
    'eg20_create_draft_name',
  );
}
export async function profileFields(fx, { creation = false, profile = fx.authority.profile } = {}) {
  const page = fx.session.page;
  await input(page, 'Agent configuration name').fill(profile.metadata.name);
  if (creation) {
    await input(page, 'Profile provider').fill(profile.metadata.provider);
    await input(page, 'Owner subject').fill(profile.metadata.owner.subject);
    await choose(page, 'Owner type', profile.metadata.owner.kind.replaceAll('_', ' '));
  }
  await input(page, 'Model').fill(profile.model.model);
  await choose(page, 'Thinking effort', profile.model.thinking_effort.replaceAll('_', ' '));
  await choose(page, 'Model access', profile.model.access);
  await input(page, 'Model service').fill(profile.model.service);
  await choose(page, 'Model API', profile.model.api_type.replaceAll('_', ' '));
  await choose(
    page,
    'Permission mode',
    profile.permission.action_mode === 'guarded' ? 'Ask before sensitive actions' : 'Autonomous',
  );
  await listField(page, 'Service scope', profile.permission.service_scope);
  await input(page, 'System prompt').fill(profile.context.system_prompt ?? '');
  while (await button(page, 'Remove skill 1').count()) await button(page, 'Remove skill 1').click();
  for (const [index, skill] of (profile.context.skills ?? []).entries()) {
    await button(page, 'Add skill').click();
    await input(page, `Skill ${index + 1} ID`).fill(skill.id);
    await input(page, `Skill ${index + 1} version`).fill(skill.version ?? '');
  }
  while (await button(page, 'Remove server 1').count())
    await button(page, 'Remove server 1').click();
  for (const [i, server] of (profile.context.mcp_servers ?? []).entries()) {
    await button(page, 'Add MCP server').click();
    await input(page, `Server ${i + 1} URL`).fill(server.url);
    await choose(page, `Server ${i + 1} authentication`, server.auth.type.replaceAll('_', ' '));
    if (server.auth.type === 'nyxid_service')
      await input(page, `Server ${i + 1} service`).fill(server.auth.slug);
  }
  if (profile.environment) {
    if (await button(page, 'Remove environment override').count())
      await button(page, 'Remove environment override').click();
    await button(page, 'Add environment settings').click();
    const section = page.getByRole('group', { name: 'Environment values', exact: true });
    for (const [key, value] of Object.entries(profile.environment)) {
      requireInput(
        ['string', 'number', 'boolean'].includes(typeof value),
        'eg20_flat_environment_fixture_required',
      );
      await input(section, 'Environment values field name').fill(key);
      await choose(section, 'Environment values value type', typeof value);
      await button(section, 'Add field').click();
      if (typeof value === 'boolean') await toggle(section, key, value);
      else await input(section, key).fill(String(value));
    }
  }
}
export async function consumerDraft(ctx, fx, { preserveProfile = false } = {}) {
  const page = fx.session.page,
    details = requireInput(fx.authority.consumer, 'eg20_consumer_seed_configuration_required');
  if (!preserveProfile) await page.goto(new URL('/bots/new', fx.session.origin).href);
  if (preserveProfile)
    ctx.assert(
      (await input(page, 'Consumer name').inputValue()) === fx.authority.fatherDraft.name,
      'eg20_father_name_prefilled_exact',
    );
  else await input(page, 'Consumer name').fill(fx.channelName);
  await choose(page, 'Event Gateway connection', details.connectionLabel);
  await button(page, 'Continue').click();
  await page
    .getByRole('radio', {
      name: new RegExp(details.imageLabel.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')),
    })
    .check();
  await input(page, 'Model').fill(details.agent.model);
  await choose(page, 'Reasoning effort', details.agent.reasoning_effort);
  await choose(
    page,
    'Permission level',
    details.agent.permission_mode === 'guarded' ? 'Guarded' : 'Autonomous',
  );
  if (preserveProfile)
    ctx.assert(
      (await input(page, 'System prompt').inputValue()) === fx.authority.fatherDraft.instructions,
      'eg20_father_prompt_prefilled_exact',
    );
  else await input(page, 'System prompt').fill(details.agent.system_prompt);
  for (const id of details.agent.permissions.services) {
    const label = requireInput(details.serviceLabels?.[id], 'eg20_service_catalog_label_required');
    await page
      .getByRole('list', { name: 'Services', exact: true })
      .getByRole('listitem')
      .filter({ has: page.getByText(label, { exact: true }) })
      .getByRole('checkbox')
      .check();
  }
  for (const skill of details.agent.skills) {
    const label = requireInput(
      details.skillLabels?.[skill.id],
      'eg20_skill_catalog_label_required',
    );
    await input(page, 'Search Ornn skills').fill(label);
    await button(page, 'Add ' + label).click();
    await choose(page, 'Version for ' + label, skill.version);
  }
  if (details.repository)
    await input(page, 'GitHub repository workspace (optional)').fill(details.repository);
  await button(page, 'Continue').click();
  await choose(page, 'Admission policy', 'Selected senders in private chats');
  await input(page, 'Sender IDs').fill(details.senderIds.join(','));
  await toggle(page, 'Send the Agent’s final answer automatically', true);
  await input(page, 'Acknowledgement delay (seconds)').fill('2');
  await button(page, 'Continue').click();
  await visible(
    ctx,
    page.getByRole('heading', { name: 'Review and create', exact: true }),
    'eg20_consumer_review_visible',
  );
  ctx.assert(await button(page, 'Create consumer').isEnabled(), 'eg20_consumer_creation_ready');
}
