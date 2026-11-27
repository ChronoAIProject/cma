import {check, id} from '../cma/core.mjs';

const timeout = 15000;

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
