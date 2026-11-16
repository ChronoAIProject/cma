import { agentPath, button, define, go, observeDOM, peer, setup, shown } from './helpers.mjs';
import {
  actualFrame,
  admit,
  desktopChoice,
  inventory,
  opened,
  repository,
  sameInventory,
  sandbox,
  taskForm,
  verifyAdmission,
} from './creation-helpers.mjs';

const draft = define('task.draft', 'browser', [
  async (ctx, s) => {
    await setup(ctx, s, 'task-draft');
    await taskForm(ctx, s);
    await button(s.page, 'Write code').click();
    ctx.assert(
      (await s.page.getByLabel('Your task', { exact: true }).inputValue()) ===
        'Write a [language] script that [does what].',
      'template_editable_draft',
    );
  },
  async (ctx, s) => {
    await s.page.getByLabel('Your task', { exact: true }).fill(s.data.draft);
    await desktopChoice(ctx, s, true);
  },
  async (ctx, s) => {
    await go(s.page, s.data.otherLocationPath);
    await button(s.page, 'More actions').click();
    await s.page.getByRole('menuitem', { name: /^New agent/ }).click();
    const locationDraft = s.page.getByRole('dialog', { name: 'New task', exact: true });
    await shown(ctx, locationDraft, 'location_task_draft_visible');
    ctx.assert(
      (await locationDraft.getByLabel('Your task', { exact: true }).inputValue()) !== s.data.draft,
      'draft_isolated_by_location',
    );
    await button(locationDraft, 'Close').click();
    await go(s.page, '/docs');
    await go(s.page, '/agent-manager');
    ctx.assert(
      (await s.page.getByLabel('Your task', { exact: true }).inputValue()) === s.data.draft,
      'draft_survives_navigation',
    );
    await desktopChoice(ctx, s, true);
  },
  async (ctx, s) => {
    await peer(ctx, s);
    ctx.assert(
      (await s.page.getByLabel('Your task', { exact: true }).inputValue()) === s.data.draft,
      'draft_survives_ingress_switch',
    );
    const outsider =
      s.data.otherSession ?? (await s.browser.open({ actor: s.data.otherActor, am: ctx.am }));
    ctx.defer('other_browser_close', () => outsider.close());
    if (outsider.page.url() === 'about:blank') await outsider.page.goto(outsider.origin);
    await go(outsider.page, '/agent-manager');
    ctx.assert(
      (await outsider.page.getByLabel('Your task', { exact: true }).inputValue()) !== s.data.draft,
      'draft_isolated_by_user',
    );
    ctx.assert(s.writes.length === 0, 'draft_never_submitted');
    await observeDOM(ctx, s, 'task_draft_ui', s.page.locator('[data-task-home]'));
  },
]);
const create = define('task.create', 'agent', [
  async (ctx, s) => {
    await setup(ctx, s, 'task-create');
    s.before = await inventory(ctx);
    await taskForm(ctx, s);
    await desktopChoice(ctx, s, false);
    await s.page.getByLabel('Your task', { exact: true }).fill(s.data.task);
  },
  async (ctx, s) => {
    await admit(ctx, s);
    ctx.assert(
      s.submitted.initial_message === s.data.task && s.submitted.machine_included !== true,
      'standard_task_payload',
    );
  },
  async (ctx, s) => {
    await opened(ctx, s);
  },
  async (ctx, s) => {
    const agent = await verifyAdmission(ctx, s),
      after = await inventory(ctx);
    for (const [field, id] of [
      ['sandboxIds', agent.sandbox.id],
      ['workspaceIds', agent.workspace.id],
      ['agentIds', agent.id],
    ]) {
      ctx.assert(
        JSON.stringify(after[field].filter((value) => !s.before[field].includes(value))) ===
          JSON.stringify([id]),
        'exactly_one_new_resource',
      );
      ctx.assert(
        s.before[field].every((value) => after[field].includes(value)),
        'unrelated_resources_preserved',
      );
    }
  },
]);
const existing = define('task.existing-location', 'agent', [
  async (ctx, s) => {
    await setup(ctx, s, 'task-existing-location');
    s.before = await inventory(ctx);
    s.runs = [];
    ctx.assert(
      s.data.locations.some((location) => location.workspaceId) &&
        s.data.locations.some((location) => !location.workspaceId),
      'both_existing_location_variants',
    );
    await taskForm(ctx, s, s.data.locations[0].path);
  },
  async (ctx, s) => {
    for (const [index, location] of s.data.locations.entries()) {
      await taskForm(ctx, s, location.path);
      const task = s.data.task + ' ' + index;
      await s.page.getByLabel('Your task', { exact: true }).fill(task);
      await admit(ctx, s);
      await opened(ctx, s);
      ctx.assert(
        location.workspaceId
          ? s.submitted.workspace_id === location.workspaceId
          : s.submitted.sandbox_id === location.sandboxId,
        'requested_existing_location',
      );
      s.runs.push({
        location,
        task,
        receipt: s.receipt,
        operation: s.operation,
        agentId: s.data.agentId,
        writes: s.writes,
      });
    }
  },
  async (ctx, s) => {
    for (const run of s.runs) {
      s.receipt = run.receipt;
      s.data.agentId = run.agentId;
      s.writes = run.writes;
      const agent = await verifyAdmission(ctx, s, run.task);
      ctx.assert(agent.sandbox.id === run.location.sandboxId, 'existing_sandbox_preserved');
      if (run.location.workspaceId)
        ctx.assert(agent.workspace.id === run.location.workspaceId, 'existing_workspace_preserved');
      await sandbox(ctx, s, run.location.sandboxId);
      ctx.assert(
        (agent.desktop === true) === run.location.machineIncluded,
        'existing_machine_setup_preserved',
      );
    }
  },
  async (ctx, s) => {
    const after = await inventory(ctx);
    ctx.assert(
      JSON.stringify(after.sandboxIds) === JSON.stringify(s.before.sandboxIds),
      'no_new_sandbox_for_existing_location',
    );
  },
]);
const repo = define('task.repository', 'agent', [
  async (ctx, s) => {
    await setup(ctx, s, 'task-repository');
    await taskForm(ctx, s);
    await button(s.page, 'Add git repo').click();
    await s.page
      .getByLabel('GitHub repository', { exact: true })
      .fill('https://invalid.example/not-allowed');
    await s.page.getByLabel('Repository ref', { exact: true }).fill('bad..ref');
    ctx.assert(await button(s.page, 'Add repo').isDisabled(), 'invalid_repository_ref_refused');
    await button(s.page, 'Cancel').click();
    await repository(ctx, s, s.data.repository.url, s.data.repository.ref);
  },
  async (ctx, s) => {
    await s.page.getByLabel('Your task', { exact: true }).fill(s.data.task);
    await admit(ctx, s);
    ctx.assert(
      s.submitted.repository.url === s.data.repository.url &&
        s.submitted.repository.ref === s.data.repository.ref,
      'repository_admission_payload',
    );
    await opened(ctx, s);
  },
  async (ctx, s) => {
    await button(s.page, 'Show Agent details').click();
    const detail = s.page.getByRole('region', { name: 'Repository context' });
    await shown(ctx, detail, 'repository_details_visible');
    for (const value of [s.data.repository.url, s.data.repository.ref, s.data.repository.commit])
      ctx.assert(
        (await detail.innerText()).includes(value),
        'repository_details_match_requested_resolved',
      );
  },
  async (ctx, s) => {
    await verifyAdmission(ctx, s);
  },
]);
const machine = define('task.machine-create', 'agent', [
  async (ctx, s) => {
    await setup(ctx, s, 'task-machine');
    await taskForm(ctx, s);
    ctx.assert(new URL(s.page.url()).pathname === '/agent-manager', 'machine_new_sandbox_context');
  },
  async (ctx, s) => {
    await desktopChoice(ctx, s, true);
  },
  async (ctx, s) => {
    await s.page.getByLabel('Your task', { exact: true }).fill(s.data.task);
    await admit(ctx, s);
    ctx.assert(s.submitted.machine_included === true, 'machine_included_sent');
  },
  async (ctx, s) => {
    await opened(ctx, s);
    await actualFrame(ctx, s);
  },
  async (ctx, s) => {
    await verifyAdmission(ctx, s);
    await actualFrame(ctx, s);
  },
]);
const manual = define('task.manual-agent', 'agent', [
  async (ctx, s) => {
    await setup(ctx, s, 'task-manual');
    await taskForm(ctx, s);
    await shown(
      ctx,
      s.page.getByText(
        'Task creation is unavailable. You can create an Agent and start a conversation.',
        { exact: true },
      ),
      'policy_off_manual_offered',
    );
    await button(s.page, 'New agent').click();
  },
  async (ctx, s) => {
    await s.page.getByLabel('GitHub repository (optional)', { exact: true }).fill('');
    await s.page.getByLabel('Include a shared desktop', { exact: true }).uncheck();
    ctx.assert(await button(s.page, 'Create agent').isEnabled(), 'manual_create_enabled');
  },
  async (ctx, s) => {
    await admit(ctx, s, 'Create agent');
    await opened(ctx, s, true);
  },
  async (ctx, s) => {
    ctx.assert(s.submitted.initial_message === undefined, 'manual_has_no_implicit_task');
    await verifyAdmission(ctx, s, null);
  },
]);
const recovery = define('task.admission-recovery', 'agent', [
  async (ctx, s) => {
    await setup(ctx, s, 'task-admission-recovery');
    await taskForm(ctx, s);
    await s.page.getByLabel('Your task', { exact: true }).fill(s.data.task);
    await s.data.faults.dropObservationAfterAdmission();
    ctx.defer('restore_admission_observation', () => s.data.faults.restore());
    await button(s.page, 'Start task').click();
    s.cut = await s.data.faults.cutReceipt();
    await s.data.custody(s.cut);
    ctx.assert(
      s.cut.accepted === true && s.cut.operationId && s.cut.idempotencyKey,
      'fault_after_actual_admission',
    );
  },
  async (ctx, s) => {
    await s.data.faults.restore();
    await peer(ctx, s, '/agent-manager');
    await s.page.waitForFunction(
      () =>
        /^\/agents\/[^/]+$/.test(location.pathname) ||
        document.querySelector('[aria-label="Submitted task"]') !== null,
    );
    const retained = s.page.getByRole('region', { name: 'Submitted task' });
    if (await retained.isVisible())
      ctx.assert((await retained.innerText()).includes(s.data.task), 'original_task_text_retained');
    else
      ctx.assert(
        new URL(s.page.url()).pathname === agentPath(s.cut.agentId),
        'recovered_route_exact_original_agent',
      );
  },
  async (ctx, s) => {
    s.receipt = { id: s.cut.operationId };
    await opened(ctx, s);
    ctx.assert(
      s.writes.every((write) => write.key === s.cut.idempotencyKey),
      'unknown_admission_never_rekeyed',
    );
  },
  async (ctx, s) => {
    await verifyAdmission(ctx, s);
  },
]);
const limits = define('task.limits', 'browser', [
  async (ctx, s) => {
    await setup(ctx, s, 'task-limits');
    await taskForm(ctx, s);
    s.before = await inventory(ctx);
    ctx.assert(
      JSON.stringify(s.data.limits.map((limit) => limit.kind).sort()) ===
        JSON.stringify(['agent', 'sandbox', 'workspace']),
      'three_resource_capacity_fixtures',
    );
    ctx.assert(
      s.data.invalidInputs.some((input) => input.kind === 'name') &&
        s.data.invalidInputs.some((input) => input.kind === 'repository-ref'),
      'invalid_name_and_ref_variants',
    );
    for (const label of s.data.displayedLimits)
      await shown(ctx, s.page.getByText(label, { exact: true }), 'configured_limit_displayed');
  },
  async (ctx, s) => {
    await s.page.getByLabel('Your task', { exact: true }).fill('x'.repeat(262145));
    await button(s.page, 'Start task').click();
    await shown(
      ctx,
      s.page.getByRole('alert').filter({ hasText: '262144-byte limit' }),
      'overlong_task_rejected',
    );
    ctx.assert(s.writes.length === 0, 'overlong_task_not_submitted');
    sameInventory(ctx, s.before, await inventory(ctx));
    // Invalid names/refs are entered in their real creation/repository UI variants.
    for (const invalid of s.data.invalidInputs) {
      const before = await inventory(ctx);
      await go(s.page, invalid.path);
      for (const field of invalid.fields)
        await s.page.getByLabel(field.label, { exact: true }).fill(field.value);
      await button(s.page, invalid.submit).click();
      await shown(
        ctx,
        s.page.getByRole('alert').filter({ hasText: invalid.errorText }),
        'invalid_name_or_ref_refused',
      );
      sameInventory(ctx, before, await inventory(ctx));
    }
    for (const limit of s.data.limits) {
      const custody = await limit.prepare();
      ctx.defer('capacity_fixture_cleanup', () => custody.cleanup());
      const before = await inventory(ctx);
      await taskForm(ctx, s, limit.locationPath);
      await s.page.getByLabel('Your task', { exact: true }).fill(s.data.task);
      await button(s.page, 'Start task').click();
      await shown(
        ctx,
        s.page.getByRole('alert').filter({ hasText: limit.errorText }),
        'resource_capacity_refusal_specific',
      );
      sameInventory(ctx, before, await inventory(ctx));
      await limit.release();
    }
  },
  async (ctx, s) => {
    await taskForm(ctx, s);
    await s.page.getByLabel('Your task', { exact: true }).fill(s.data.task);
    ctx.assert(await button(s.page, 'Start task').isEnabled(), 'corrected_input_available');
  },
  async (ctx, s) => {
    ctx.assert(
      new Set(s.data.limits.map((limit) => limit.errorText)).size === 3,
      'capacity_refusals_distinguished',
    );
    await observeDOM(ctx, s, 'corrected_task_ui', s.page.locator('[data-task-home]'));
  },
]);
export const cases = [draft, create, existing, repo, machine, manual, recovery, limits];
