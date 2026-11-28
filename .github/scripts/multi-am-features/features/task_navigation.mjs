import {randomUUID} from 'node:crypto';
import {check, id} from '../cma/core.mjs';
import {withRestoration} from '../failure.mjs';

const timeout = 15000;

export async function taskDraft(page, {workspaceId}, {owner, recipient, recipientCookie}) {
  check(id(workspaceId), 'browser_draft_workspace_required');
  check(id(owner) && id(recipient) && owner !== recipient && typeof recipientCookie === 'string'
    && recipientCookie.length > 0, 'browser_draft_distinct_accounts_required');
  const draft = 'Keep this fixture draft ' + randomUUID();
  const workspaceDraft = 'Keep this Workspace draft ' + randomUUID();
  const newTask = '/agent-manager';
  const workspace = `/workspaces/${workspaceId}`;
  const visit = path => page.goto(path, {waitUntil: 'domcontentloaded', timeout});
  const field = page.getByLabel('Your task', {exact: true});
  const desktop = page.getByRole('button', {name: /^Cloud computer/});
  const admission = url => /^\/api\/(?:agent-manager\/)?v1\/(?:agents|sandboxes|workspaces\/[^/]+\/agents)$/.test(url.pathname);
  let submissions = 0;
  const rejectSubmission = async route => {
    if (route.request().method() !== 'POST') return route.fallback();
    submissions += 1;
    await route.abort('blockedbyclient');
  };
  await page.route(admission, rejectSubmission);
  try {
    await visit(newTask);
    await page.getByRole('button', {name: 'Templates', exact: true}).click({timeout});
    await page.getByRole('menuitem', {name: 'Write code', exact: true}).click({timeout});
    await page.getByLabel('Language', {exact: true}).fill('Python', {timeout});
    await page.getByLabel('What should the code do?', {exact: true}).fill('Format synthetic weather readings.', {timeout});
    await page.getByRole('button', {name: 'Write a task without a template', exact: true}).click({timeout});
    await field.waitFor({timeout});
    const preview = await field.inputValue();
    check(preview.includes('Python') && preview.includes('Format synthetic weather readings.'),
      'browser_draft_template_preview_missing');
    await field.fill(draft, {timeout});
    await desktop.click({timeout});
    await page.getByRole('menuitemcheckbox', {name: 'Shared desktop', exact: true, checked: false}).click({timeout});
    await page.keyboard.press('Escape');
    await page.getByRole('button', {name: 'Cloud computer · Shared desktop', exact: true}).waitFor({timeout});

    const scoped = page.getByRole('dialog', {name: 'New task', exact: true});
    const scopedDraft = scoped.getByLabel('Your task', {exact: true});
    const openWorkspaceDraft = async () => {
      await visit(workspace);
      await page.getByRole('button', {name: 'New agent in this Workspace', exact: true}).click({timeout});
      await scopedDraft.waitFor({timeout});
    };
    await openWorkspaceDraft();
    check(await scopedDraft.inputValue() === '', 'browser_draft_leaked_to_workspace');
    await scopedDraft.fill(workspaceDraft, {timeout});
    await scoped.getByRole('button', {name: 'Close', exact: true}).click({timeout});

    await visit('/docs');
    await visit(newTask);
    await field.waitFor({timeout});
    check(await field.inputValue() === draft, 'browser_draft_text_lost_after_navigation');
    await page.getByRole('button', {name: 'Cloud computer · Shared desktop', exact: true}).waitFor({timeout});
    await openWorkspaceDraft();
    check(await scopedDraft.inputValue() === workspaceDraft, 'browser_workspace_draft_lost_after_navigation');

    const context = page.context();
    const host = new URL(page.url()).hostname;
    const [ownerCookie] = (await context.cookies()).filter(cookie => cookie.name === 'cma_session'
      && cookie.domain === host && cookie.path === '/');
    check(ownerCookie?.httpOnly && ownerCookie.sameSite === 'Lax' && ownerCookie.value !== recipientCookie,
      'browser_draft_distinct_sessions_required');
    const switchAccount = async (cookie, subject) => {
      await context.addCookies([cookie]);
      await visit(newTask);
      const state = await page.evaluate(async deadline => {
        const response = await fetch('/auth/session', {credentials: 'same-origin', redirect: 'error',
          cache: 'no-store', signal: AbortSignal.timeout(deadline)});
        if (!response.ok) return null;
        const {authenticated, subject} = await response.json();
        return {authenticated, subject};
      }, timeout);
      check(state?.authenticated === true && state.subject === subject, 'browser_draft_account_switch_unbound');
      await field.waitFor({timeout});
    };
    try {
      await switchAccount({...ownerCookie, value: recipientCookie}, recipient);
      check(await field.inputValue() === '', 'browser_draft_leaked_to_recipient');
      await field.fill('The other account has its own fixture draft.', {timeout});
    } finally {
      await switchAccount(ownerCookie, owner);
    }
    check(await field.inputValue() === '', 'browser_draft_retained_after_account_switch');
    check(submissions === 0, 'browser_draft_submitted_without_confirmation');
  } finally {
    await page.unroute(admission, rejectSubmission);
  }
}

export async function taskNavigation(page, {agentId, sandboxId, workspaceId}) {
  check([agentId, sandboxId, workspaceId].every(id), 'browser_task_lineage_required');
  const agentPath = `/agents/${agentId}`;
  const views = page.getByRole('group', {name: 'Navigation view', exact: true});
  const taskView = views.getByRole('button', {name: 'Tasks', exact: true});
  const environmentView = views.getByRole('button', {name: 'Environments', exact: true});
  const row = (type, key) => page.locator(`[role="treeitem"][data-node-id="${type}:${key}"]:visible`);
  const taskRow = row('agent', agentId);
  const taskLink = taskRow.locator(`a[href="${agentPath}"]`);
  const exactTask = async () => {
    await taskLink.waitFor({timeout});
    check(await taskRow.count() === 1, 'browser_task_list_duplicate');
  };

  await page.goto('/agent-manager', {waitUntil: 'domcontentloaded', timeout});
  await taskView.click({timeout});
  await exactTask();
  const detailResponse = await page.request.get('/api/v1' + agentPath, {timeout});
  check(detailResponse.ok(), 'browser_task_detail_unavailable');
  const detail = await detailResponse.json();
  check(detail.id === agentId && detail.sandbox?.id === sandboxId && detail.workspace?.id === workspaceId
    && detail.status === 'ready' && detail.sandbox.status === 'active', 'browser_task_detail_lineage_changed');
  const summaryResponse = await page.request.get(`/api/v1/workspaces/${workspaceId}`, {timeout});
  check(summaryResponse.ok(), 'browser_task_workspace_unavailable');
  const summary = (await summaryResponse.json()).agents?.find(agent => agent.id === agentId);
  const label = summary?.task_status === 'not_started' ? 'Ready to start'
    : summary?.task_status === 'completed' ? 'Turn ended' : null;
  check(summary?.status === 'ready' && label, 'browser_task_summary_unbound');
  await taskRow.getByText(label, {exact: true}).waitFor({timeout});

  await taskLink.click({timeout});
  await page.waitForURL(url => url.pathname === agentPath, {timeout});
  await environmentView.click({timeout});
  const expand = async (type, key) => {
    const branch = row(type, key);
    await branch.waitFor({timeout});
    if (await branch.getAttribute('aria-expanded', {timeout}) !== 'true') {
      await branch.getByRole('button', {name: /^Expand /}).click({timeout});
    }
    check(await branch.getAttribute('aria-expanded', {timeout}) === 'true', 'browser_task_branch_not_expanded');
  };
  await expand('sandbox', sandboxId);
  await expand('workspace', workspaceId);
  await exactTask();
  check(await taskRow.getAttribute('data-parent-id', {timeout}) === `workspace:${workspaceId}`,
    'browser_task_workspace_unbound');
  await taskView.click({timeout});
  await page.waitForURL(url => url.pathname === agentPath, {timeout});

  let failedReads = 0;
  const sandboxList = url => url.pathname === '/api/v1/sandboxes';
  await page.route(sandboxList, async route => {
    if (route.request().method() !== 'GET') return route.fallback();
    failedReads += 1;
    return route.fulfill({status: 503, json: {status: 503, title: 'Fixture navigation read unavailable'}});
  });
  try {
    await page.reload({waitUntil: 'domcontentloaded', timeout});
    const retry = page.getByRole('alert').getByRole('button', {name: 'Try again', exact: true});
    await retry.waitFor({timeout});
    check(failedReads > 0, 'browser_task_navigation_fault_not_consumed');
    await page.unroute(sandboxList);
    await retry.click({timeout});
    await exactTask();
    await page.waitForURL(url => url.pathname === agentPath, {timeout});
  } finally {
    await page.unroute(sandboxList);
  }
}

export async function agentOpen(page, {agentId}, {input, answer}) {
  check(id(agentId) && typeof input === 'string' && input.length > 0
    && typeof answer === 'string' && answer.length > 0, 'browser_open_turn_witness_required');
  const agentPath = `/agents/${agentId}`;
  const views = page.getByRole('group', {name: 'Navigation view', exact: true});
  const task = page.getByRole('treeitem').locator(`a[href="${agentPath}"]`);
  const composer = page.getByRole('form', {name: 'Message Agent', exact: true});
  const message = composer.getByRole('textbox', {name: 'Message', exact: true});
  const send = composer.getByRole('button', {name: 'Send message', exact: true});
  for (let visit = 0; visit < 2; visit++) {
    await page.goto('/agent-manager', {waitUntil: 'domcontentloaded', timeout});
    await views.getByRole('button', {name: 'Tasks', exact: true}).click({timeout});
    await task.click({timeout});
    await page.waitForURL(url => url.pathname === agentPath, {timeout});
    await page.getByText(input, {exact: true}).waitFor({timeout});
    await page.getByText(answer, {exact: true}).waitFor({timeout});
    await message.fill('Check whether the composer is ready.', {timeout});
    await send.click({trial: true, timeout});
    await message.fill('', {timeout});
  }
}

export async function agentRename({page, api, ingress, agentId, input, answer, recordFailure}) {
  check(id(agentId), 'browser_rename_agent_required');
  const path = '/agents/' + agentId;
  const preferencesPath = '/api/v1' + path + '/preferences';
  const peer = ingress === 'a' ? 'b' : 'a';
  const read = async am => (await api.request(am, 'GET', preferencesPath, {direct: true})).body;
  const rename = async (am, revision, title) => (await api.request(am, 'PATCH', preferencesPath, {
    direct: true, key: randomUUID(), body: {expected_revision: revision, title},
  })).body;
  const before = await read(peer);
  check(typeof before.title === 'string' && before.title.length > 0 && Number.isSafeInteger(before.revision),
    'browser_rename_original_preferences_unbound');
  const concurrentTitle = 'Peer rename ' + randomUUID();
  const newTitle = 'Browser rename ' + randomUUID();
  const dialog = page.getByRole('dialog', {name: 'Rename Agent', exact: true});
  const name = dialog.getByRole('textbox', {name: 'Agent name', exact: true});
  const revision = value => dialog.getByText(`Current preferences revision: ${value}.`, {exact: true});
  const save = async (status, expectedRevision) => {
    const [response] = await Promise.all([
      page.waitForResponse(response => new URL(response.url()).pathname === preferencesPath
        && response.request().method() === 'PATCH', {timeout}),
      dialog.getByRole('button', {name: 'Save', exact: true}).click({timeout}),
    ]);
    check(response.status() === status, 'browser_rename_unexpected_patch_status');
    const submitted = response.request().postDataJSON();
    check(submitted?.title === newTitle && submitted.expected_revision === expectedRevision,
      'browser_rename_submitted_different_intent');
    return response.json();
  };
  const transcript = async () => {
    await page.getByText(input, {exact: true}).waitFor({timeout});
    await page.getByText(answer, {exact: true}).waitFor({timeout});
  };
  await withRestoration(async () => {
    await page.waitForURL(url => url.pathname === path, {timeout});
    await transcript();
    const composer = page.getByRole('form', {name: 'Message Agent', exact: true});
    const originalComposer = await composer.elementHandle({timeout});
    await page.getByRole('button', {name: 'More Agent actions', exact: true}).click({timeout});
    await page.getByRole('menuitem', {name: 'Rename', exact: true}).click({timeout});
    await revision(before.revision).waitFor({timeout});
    await name.fill(newTitle, {timeout});
    const changed = await rename(peer, before.revision, concurrentTitle);
    check(changed.title === concurrentTitle && changed.revision === before.revision + 1,
      'browser_rename_peer_update_missing');
    const stale = await save(409, before.revision);
    check(stale.settings_update_reason === 'stale_revision', 'browser_rename_stale_result_untyped');
    await dialog.getByRole('alert').getByText(/Preferences changed before this update/).waitFor({timeout});
    const refused = await read(peer);
    check(refused.title === concurrentTitle && refused.revision === changed.revision,
      'browser_rename_stale_write_changed_title');

    await dialog.getByRole('button', {name: 'Reload preferences', exact: true}).click({timeout});
    await revision(changed.revision).waitFor({timeout});
    await name.fill(newTitle, {timeout});
    const {title, revision: acknowledged} = await save(200, changed.revision);
    check(title === newTitle && acknowledged === changed.revision + 1, 'browser_rename_receipt_changed');
    await dialog.waitFor({state: 'hidden', timeout});
    await page.getByRole('heading', {name: newTitle, exact: true}).waitFor({timeout});
    check(await composer.evaluate((node, original) => node === original && original.isConnected,
      originalComposer, {timeout}), 'browser_rename_remounted_conversation');
    await transcript();

    await page.reload({waitUntil: 'domcontentloaded', timeout});
    await page.waitForURL(url => url.pathname === path, {timeout});
    await page.getByRole('heading', {name: newTitle, exact: true}).waitFor({timeout});
    await page.getByRole('treeitem').locator(`a[href="${path}"]`).getByText(newTitle, {exact: true}).waitFor({timeout});
    await transcript();
    const retained = await read(peer);
    check(retained.title === newTitle && retained.revision === acknowledged, 'browser_rename_peer_readback_changed');
  }, async () => {
    const current = await read(peer);
    if (current.title === before.title) return;
    check([concurrentTitle, newTitle].includes(current.title), 'browser_rename_restore_foreign_title');
    const restored = await rename(peer, current.revision, before.title);
    check(restored.title === before.title && restored.revision === current.revision + 1,
      'browser_rename_restore_not_acknowledged');
    const observed = await read(ingress);
    check(observed.title === restored.title && observed.revision === restored.revision,
      'browser_rename_restore_not_visible');
  }, recordFailure);
}

export async function copyAgentResponse(page, {answer, code, markdown}) {
  await page.context().grantPermissions(['clipboard-read', 'clipboard-write']);
  const message = page.getByRole('article').filter({has: page.getByText(answer, {exact: true})});
  const clipboard = () => page.evaluate(() => navigator.clipboard.readText());
  const copyMessage = message.getByRole('button', {name: 'Copy content', exact: true});
  const copyCode = message.getByRole('button', {name: 'Copy code', exact: true});
  await page.evaluate(() => Object.defineProperty(navigator.clipboard, 'writeText', {
    configurable: true, value: () => Promise.reject(new DOMException('Fixture clipboard refusal', 'NotAllowedError')),
  }));
  try {
    await copyMessage.click({timeout});
    await message.getByText('Copy failed. Use Copy content to retry.', {exact: true}).waitFor({timeout});
  } finally {
    await page.evaluate(() => Reflect.deleteProperty(navigator.clipboard, 'writeText'));
  }
  await copyMessage.click({timeout});
  await message.getByRole('status').getByText('Copied', {exact: true}).waitFor({timeout});
  check(await clipboard() === markdown, 'browser_copy_message_content_changed');
  await copyCode.click({timeout});
  await message.getByRole('status').getByText('Code copied', {exact: true}).waitFor({state: 'attached', timeout});
  check(await clipboard() === code, 'browser_copy_code_content_changed');
}

export async function commandSearch(page, active, retired) {
  const targets = [['agents', active.agentId], ['sandboxes', active.sandboxId], ['workspaces', active.workspaceId]];
  check([...targets.map(([, key]) => key), retired.agentId, retired.sandboxId].every(id),
    'browser_search_lineage_required');
  await page.goto('/agent-manager', {waitUntil: 'domcontentloaded', timeout});
  await page.getByRole('button', {name: 'Search', exact: true}).click({timeout});
  const field = page.getByRole('combobox', {name: 'Search commands'});
  const results = page.getByRole('listbox', {name: 'Results'});
  for (const command of ['New agent', 'Agent Manager', 'Event Gateway']) {
    await field.fill(command, {timeout});
    await results.getByRole('option', {name: command, exact: true}).waitFor({timeout});
  }
  for (const [kind, key] of targets) {
    await field.fill(key, {timeout});
    const match = results.getByRole('option');
    await match.first().waitFor({timeout});
    check(await match.count() === 1, 'browser_search_duplicate_resource');
    await match.click({timeout});
    await page.waitForURL(url => url.pathname === `/${kind}/${key}`, {timeout});
    await field.waitFor({state: 'hidden', timeout});
    await page.keyboard.press('ControlOrMeta+k');
    await field.waitFor({timeout});
  }

  for (const key of [retired.agentId, retired.sandboxId]) {
    await field.fill(key, {timeout});
    await results.getByText('No results.', {exact: true}).waitFor({timeout});
  }
  await field.fill('Operations', {timeout});
  await results.getByText('No results.', {exact: true}).waitFor({timeout});
}
