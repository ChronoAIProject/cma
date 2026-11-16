import {
  agentPath,
  browserEvidence,
  button,
  go,
  peer,
  readAgent,
  ready,
  shown,
  transcript,
  uiWrite,
  watchWrites,
} from './helpers.mjs';
import { until } from './turn-helpers.mjs';

export const admissionPath = '/api/agent-manager/v1/agents';
export async function taskForm(ctx, s, path = '/agent-manager') {
  await go(s.page, path);
  await shown(ctx, s.page.getByLabel('Your task', { exact: true }), 'task_input_visible');
  s.writes ??= watchWrites(ctx, s, admissionPath);
}
export async function desktopChoice(ctx, s, enabled) {
  await button(s.page, /^Cloud computer/).click();
  const choice = s.page.getByRole('menuitemcheckbox', { name: 'Shared desktop', exact: true });
  if (((await choice.getAttribute('aria-checked')) === 'true') !== enabled) await choice.click();
  ctx.assert(
    (await choice.getAttribute('aria-checked')) === String(enabled),
    'shared_desktop_choice',
  );
  await s.page.keyboard.press('Escape');
}
export async function admit(ctx, s, label = 'Start task') {
  s.admissionStarted = performance.now();
  const result = await uiWrite(ctx, s, 'POST', admissionPath, () => button(s.page, label).click());
  s.receipt = result.body;
  s.submitted = result.submitted;
  await s.data.custody(s.receipt);
  ctx.assert(
    typeof s.receipt.id === 'string' && s.receipt.id.length > 0,
    'admission_operation_identity',
  );
}
export async function opened(ctx, s, manual = false) {
  if (manual) await button(s.page, 'Open agent').click();
  await s.page.waitForURL((url) => /^\/agents\/[^/]+$/.test(url.pathname), { timeout: 120000 });
  s.data.agentId = decodeURIComponent(new URL(s.page.url()).pathname.split('/').at(-1));
  await s.data.custody({ agent_id: s.data.agentId, operation_id: s.receipt.id });
  await ready(ctx, s);
  if (s.admissionStarted !== undefined)
    await ctx.observe('create_to_usable_timing', async () => ({
      elapsed_ms: performance.now() - s.admissionStarted,
      target_ms: 5000,
      agent_id: s.data.agentId,
    }));
  const operation = await ctx.request({
    method: 'GET',
    path: '/api/agent-manager/v1/agent-lifecycle/' + encodeURIComponent(s.receipt.id),
    agentId: s.data.agentId,
  });
  ctx.assert(
    operation.body.id === s.receipt.id && operation.body.agent_id === s.data.agentId,
    'admission_opened_exact_agent',
  );
  ctx.assert(
    ['running', 'succeeded'].includes(operation.body.status),
    'admission_progress_operable',
  );
  s.operation = operation.body;
}
export async function verifyAdmission(ctx, s, initial = s.data.task) {
  await peer(ctx, s, agentPath(s.data.agentId));
  await ready(ctx, s);
  const agent = await readAgent(ctx, s);
  ctx.assert(agent.bootstrap?.operation_id === s.receipt.id, 'agent_bootstrap_operation_identity');
  const writes =
    initial === null
      ? s.writes
      : s.writes.filter((write) => write.body.initial_message === initial);
  ctx.assert(
    writes.length > 0 && writes.every((write) => write.key && write.key === writes[0].key),
    'one_admission_intent',
  );
  if (initial !== null) {
    const turns = await until(
      ctx,
      () => transcript(ctx, s),
      (rows) => rows.some((turn) => turn.owner_message === initial),
      'initial_message_observed',
    );
    ctx.assert(
      turns.filter((turn) => turn.owner_message === initial).length === 1,
      'initial_message_exactly_once',
    );
  }
  await browserEvidence(ctx, s);
  return agent;
}
export async function repository(ctx, s, url, ref) {
  await button(s.page, 'Add git repo').click();
  await s.page.getByLabel('GitHub repository', { exact: true }).fill(url);
  await s.page.getByLabel('Repository ref', { exact: true }).fill(ref);
  await button(s.page, 'Add repo').click();
  ctx.assert(
    (await s.page.locator('.task-home__repository-slot').innerText()).length > 0,
    'repository_choice_visible',
  );
}
export async function sandbox(ctx, s, id) {
  const result = await ctx.request({
    method: 'GET',
    path: '/api/v1/sandboxes/' + encodeURIComponent(id),
  });
  ctx.assert(result.body.id === id, 'exact_sandbox_identity');
  return result.body;
}
export async function inventory(ctx) {
  const sandboxes = await ctx.request({ method: 'GET', path: '/api/v1/sandboxes?limit=100' });
  const agents = await ctx.request({ method: 'GET', path: '/api/v1/agents?limit=100' });
  ctx.assert(!sandboxes.body.next && !agents.body.next, 'fixture_inventory_bounded');
  const workspaces = [];
  for (const item of sandboxes.body.items) {
    const page = await ctx.request({
      method: 'GET',
      path: '/api/v1/sandboxes/' + encodeURIComponent(item.id) + '/workspaces?limit=100',
    });
    ctx.assert(!page.body.next, 'fixture_workspace_inventory_bounded');
    workspaces.push(...page.body.items);
  }
  return {
    sandboxIds: sandboxes.body.items.map((item) => item.id).sort(),
    workspaceIds: workspaces.map((item) => item.id).sort(),
    agentIds: agents.body.items.map((item) => item.id).sort(),
  };
}
export function sameInventory(ctx, before, after) {
  ctx.assert(JSON.stringify(before) === JSON.stringify(after), 'refusal_did_not_admit_resources');
}
export async function actualFrame(ctx, s) {
  const frame = await s.data.desktop.observe(s.data.agentId);
  ctx.assert(
    frame.agentId === s.data.agentId &&
      frame.sequence > 0 &&
      frame.width > 0 &&
      frame.height > 0 &&
      /^[a-f0-9]{64}$/.test(frame.sha256),
    'actual_desktop_frame',
  );
  ctx.assert(
    frame.os === 'linux' && frame.processes.includes('chromium'),
    'linux_chromium_runtime',
  );
  await shown(
    ctx,
    s.page.getByRole('region', { name: 'Machine desktop' }).locator('canvas'),
    'desktop_canvas_rendered',
  );
  await ctx.observe('rendered_desktop_frame', async () => frame);
}
