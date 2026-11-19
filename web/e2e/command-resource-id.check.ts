import { test, expect, type Page } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';

async function setup(page: Page, baseURL: string, targetId = 'agt_lookup_alpha') {
  const boundary = new LocalStartup();
  const first = boundary.workspace.agents[0]!;
  const agents = [
    ...Array.from({ length: 65 }, (_, index) => ({
      ...first,
      id: `agt_recent_${index}`,
      title: `Recent task ${index}`,
    })),
    ...['alpha', 'beta'].map((suffix) => ({
      ...first,
      id: `agt_lookup_${suffix}`,
      title: 'Review code',
      updated_at: '2026-09-01T00:00:00Z',
    })),
    { ...first, id: 'agt_lookup_gamma', title: 'Plan rollout', updated_at: '2026-09-01T00:00:00Z' },
    { ...first, id: 'agt_deleted', title: 'Deleted task', status: 'deleted' as const },
  ];
  boundary.workspace = { ...boundary.workspace, agents, agent_count: agents.length };
  boundary.sandbox = { ...boundary.sandbox, name: 'Local lab', agent_count: agents.length };
  boundary.agent = { ...boundary.agent, id: targetId, title: 'Review code' };
  const requests: string[] = [];
  page.on('request', (request) => {
    const url = new URL(request.url());
    if (url.pathname.startsWith('/api/'))
      requests.push(`${request.method()} ${url.pathname}${url.search}`);
  });
  await boundary.route(page, baseURL);
  await page.route(/\/api\/v1\/agents\/[^/?]+$/, (route) => {
    expect(route.request().method()).toBe('GET');
    const id = new URL(route.request().url()).pathname.split('/').at(-1)!;
    const agent = agents.find((item) => item.id === id);
    if (!agent) return route.fallback();
    return route.fulfill({
      json: { ...boundary.agent, id, title: agent.title, status: agent.status },
    });
  });
  await page.route(/\/api\/v1\/agents\/[^/]+\/(?:transcript|usage)$/, (route) => {
    expect(route.request().method()).toBe('GET');
    const path = new URL(route.request().url()).pathname;
    const id = path.split('/').at(-2)!;
    if (!agents.some((agent) => agent.id === id)) return route.fallback();
    return route.fulfill({
      json: path.endsWith('/transcript')
        ? { agent_id: id, through_seq: 0, turns: [] }
        : { agent_id: id, through_seq: 0, input_tokens: 0, output_tokens: 0, context_used: 0 },
    });
  });
  await page.route('**/api/v1/resource-access/workspace/wks_route', (route) =>
    route.fulfill({
      json: {
        capabilities: ['view_details', 'manage_sharing'],
        owner: { kind: 'person', is_current_person: true },
        revalidate_at: null,
        policy_revision: 1,
      },
    }),
  );
  const metadata = new Set<string>();
  await page.route(/\/api\/v1\/agents\/[^/]+\/(?:channel-usage|provenance)$/, async (route) => {
    expect(route.request().method()).toBe('GET');
    const path = new URL(route.request().url()).pathname;
    const id = path.split('/').at(-2)!;
    if (!agents.some((agent) => agent.id === id)) return route.fallback();
    await route.fulfill({
      json: path.endsWith('/channel-usage')
        ? {
            agent_id: id,
            channel_usage: {
              active: false,
              has_history: false,
              first_used_at: null,
              last_used_at: null,
            },
          }
        : {
            agent_id: id,
            created_at: boundary.agent.created_at,
            creation_origin: 'legacy_unknown',
          },
    });
    metadata.add(path);
  });
  await page.clock.install();
  await page.goto('/agent-manager');
  await expect(page.getByRole('button', { name: 'Search', exact: true })).toBeEnabled();
  await page.keyboard.press('Control+k');
  const search = page.getByRole('combobox', { name: 'Search commands' });
  await expect(search).toBeFocused();
  await search.fill('Review code');
  await expect(page.getByRole('option')).toHaveCount(2);
  await search.fill('');
  await expect(page.getByRole('option')).toHaveCount(60);
  await expect.poll(() => metadata.size).toBe(2 * (agents.length - 1));
  // Hold background poll clocks after navigation has loaded to isolate menu reads.
  await page.clock.pauseAt(await page.evaluate(() => Date.now() + 1000));
  requests.length = 0;
  return { boundary, requests, search };
}

for (const [id, choice] of [
  ['agt_lookup_alpha', 'Enter'],
  ['agt_lookup_beta', 'pointer'],
] as const) {
  test(`LOCAL canonical Agent ID beyond the initial result limit navigates by ${choice}`, async ({
    page,
    baseURL,
  }) => {
    const { boundary, requests, search } = await setup(page, baseURL!, id);
    for (const query of [
      id,
      `  ${id.toUpperCase()}  `,
      id.replace('agt_lookup', ''),
      `Review ${id.slice(-4)}`,
    ]) {
      await search.fill(query);
      await expect(page.getByRole('option')).toHaveCount(1);
      await expect(page.getByRole('option')).toContainText('Review code');
      await expect(page.getByRole('option')).not.toContainText(id);
    }
    await search.fill('Plan rollout');
    await expect(page.getByRole('option')).toHaveCount(1);
    await expect(page.getByRole('option')).toContainText('Plan rollout');
    await search.fill(`Plan ${id}`);
    await expect(page.getByText('No results.', { exact: true })).toBeVisible();
    await search.fill(id);
    await search.press('Escape');
    await expect(search).toHaveCount(0);
    await page.keyboard.press('Control+k');
    await expect(search).toHaveValue(id);
    await search.fill(id);
    expect(requests).toEqual([]);
    await page.clock.resume();
    if (choice === 'Enter') await search.press('Enter');
    else await page.getByRole('option').click();
    await expect(page).toHaveURL(`${baseURL}/agents/${id}`);
    await expect(search).toHaveCount(0);
    await expect(page.getByRole('heading', { name: 'Review code', exact: true })).toBeVisible();
    expect(boundary.writes).toEqual([]);
    expect(boundary.unexpected).toEqual([]);
    expect(boundary.errors).toEqual([]);
  });
}

for (const [query, path] of [
  ['wks_route', '/workspaces/wks_route'],
  ['sbx_route', '/sandboxes/sbx_route'],
] as const) {
  test(`LOCAL ${query} resolves its canonical resource route`, async ({ page, baseURL }) => {
    const { boundary, requests, search } = await setup(page, baseURL!);
    await search.fill('Local lab Live');
    await expect(page.getByRole('option')).toHaveCount(1);
    await expect(page.getByRole('option')).toContainText('Local lab');
    await search.fill(query);
    await expect(page.getByRole('option')).toHaveCount(1);
    expect(requests).toEqual([]);
    await page.clock.resume();
    await page.getByRole('option').click();
    await expect(page).toHaveURL(`${baseURL}${path}`);
    await expect(search).toHaveCount(0);
    expect(boundary.writes).toEqual([]);
    expect(boundary.unexpected).toEqual([]);
    expect(boundary.errors).toEqual([]);
  });
}

test('LOCAL ID searches neither discover absent resources nor run internal actions', async ({
  page,
  baseURL,
}) => {
  const { boundary, requests, search } = await setup(page, baseURL!);
  for (const query of [
    'agt_deleted',
    'agt_not_visible',
    'wks_not_visible',
    'sbx_not_visible',
    'action:new-agent',
    'go:agent-manager',
  ]) {
    await search.fill(query);
    await expect(page.getByText('No results.', { exact: true })).toBeVisible();
    await search.press('Enter');
    await expect(page).toHaveURL(`${baseURL}/agent-manager`);
  }
  expect(requests).toEqual([]);
  expect(boundary.writes).toEqual([]);
  expect(boundary.unexpected).toEqual([]);
  expect(boundary.errors).toEqual([]);
});
