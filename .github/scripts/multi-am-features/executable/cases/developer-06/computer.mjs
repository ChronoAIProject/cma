import { define, setup, button, visible, contains, absent, capture, enc, hash, until } from './helpers.mjs';
const ap = (ctx) => `/api/v1/agents/${enc(ctx.state.f.agentId)}`;
export const cases = [
  define('browser.control', [
    async (ctx) => {
      const { page, origin, f } = await setup(ctx, 'browser-control');
      await page.goto(origin + '/agents/' + enc(f.agentId));
      await button(page, 'Show Agent computer').click();
      await page.getByRole('tab', { name: 'Browser', exact: true }).click();
      const r = await capture(page, 'POST', ap(ctx) + '/browser/control', () =>
        button(page, 'Take control').click(),
      );
      ctx.assert(
        r.status === 200 && r.body.state === 'user' && r.body.runtime_incarnation === f.runtimeIncarnation,
        'current_runtime_control',
      );
      ctx.state.control = r.body;
    },
    async (ctx) => {
      const page = ctx.state.b.page,
        f = ctx.state.f,
        input = page.getByRole('application', { name: 'Agent browser input', exact: true });
      await input.locator('img').click({ position: f.clickPosition });
      await input.press('Enter');
      await input.press('End');
      await page.getByLabel('Text to type in browser', { exact: true }).fill(f.textMarker);
      await button(page, 'Type text').click();
      await until(async () => {
        ctx.state.inputs = await f.inputJournal();
        return ctx.state.inputs.some((e) => e.text_sha256 === hash(f.textMarker));
      });
      ctx.assert(
        ctx.state.inputs.some((e) => e.kind === 'click') &&
          ctx.state.inputs.some((e) => e.key === 'Enter') &&
          ctx.state.inputs.some((e) => e.scroll_y > f.initialScrollY),
        'click_key_scroll_observed',
      );
    },
    async (ctx) => {
      const r = await capture(ctx.state.b.page, 'POST', ap(ctx) + '/browser/control', () =>
        button(ctx.state.b.page, 'Return to Agent').click(),
      );
      ctx.assert(r.body.state === 'agent', 'control_returned');
      ctx.assert(
        (await ctx.state.b.page
          .getByRole('application', { name: 'Agent browser input' })
          .getAttribute('aria-disabled')) === 'true',
        'input_disabled_without_control',
      );
    },
    async (ctx) => {
      const page = ctx.state.b.page;
      await button(page, 'Take control').click();
      await visible(ctx, button(page, 'Return to Agent'), 'control_reacquired');
      const fault = await ctx.state.f.dropNextResponse({
        method: 'POST',
        path: ap(ctx) + '/browser/control',
        afterCommit: true,
      });
      ctx.defer('remove_browser_fault', () => fault.close());
      await page.getByLabel('Text to type in browser', { exact: true }).fill(ctx.state.f.uncertainMarker);
      await button(page, 'Type text').click();
      await button(page, 'Check original operation').click();
      // Retry is only the UI's retained exact operation, after the status read.
      if (await button(page, 'Retry exact operation').isVisible())
        await button(page, 'Retry exact operation').click();
      await visible(ctx, button(page, 'Return to Agent'), 'browser_operation_recovered');
      const journal = await ctx.state.f.inputJournal();
      const matching = journal.filter((e) => e.text_sha256 === hash(ctx.state.f.uncertainMarker));
      ctx.assert(
        matching.length === 1 && matching[0].runtime_incarnation === ctx.state.f.runtimeIncarnation,
        'uncertain_input_once_same_runtime',
      );
      ctx.state.recovered = matching[0];
    },
    async (ctx) => {
      const journal = await ctx.state.f.controlJournal();
      const same = journal.filter((e) => e.operation_id === ctx.state.recovered.operation_id);
      ctx.assert(
        same.filter((e) => e.stage === 'effect').length === 1 && same.some((e) => e.stage === 'status_read'),
        'original_operation_checked_before_retry',
      );
      ctx.assert(
        journal.every((e) => !e.effect || e.authorized === true),
        'all_inputs_current_authority',
      );
      await button(ctx.state.b.page, 'Return to Agent').click();
      await ctx.observe('browser_control_journal', async () =>
        journal.map((e) => ({
          operation_sha256: hash(e.operation_id),
          stage: e.stage,
          authorized: e.authorized,
        })),
      );
    },
  ]),
  define('machine.terminal-output', [
    async (ctx) => {
      const { page, origin, f } = await setup(ctx, 'terminal-output');
      await page.goto(origin + '/agents/' + enc(f.agentId));
      await page.getByRole('textbox', { name: 'Message', exact: true }).fill(f.prompt);
      await button(page, 'Send message').click();
      await contains(ctx, page.locator('main'), f.replyMarker, 'harmless_command_complete');
    },
    async (ctx) => {
      await button(ctx.state.b.page, 'Show Agent computer').click();
      await ctx.state.b.page.getByRole('tab', { name: 'Terminal', exact: true }).click();
      await visible(
        ctx,
        ctx.state.b.page.getByLabel('Recorded command', { exact: true }),
        'recorded_command_visible',
      );
    },
    async (ctx) => {
      const f = ctx.state.f;
      await contains(
        ctx,
        ctx.state.b.page.getByLabel('Recorded command', { exact: true }),
        f.command,
        'recorded_command_exact',
      );
      for (const marker of [f.stdoutMarker, f.stderrMarker])
        await contains(
          ctx,
          ctx.state.b.page.getByLabel('Recorded output', { exact: true }),
          marker,
          'recorded_output_exact',
        );
      const journal = await f.operationJournal();
      ctx.assert(
        journal.agent_id === f.agentId && journal.exit_code === 0 && journal.finished_at && journal.turn_id,
        'recorded_completion_bound',
      );
      ctx.state.operation = journal;
    },
    async (ctx) => {
      const id = ctx.state.operation.operation_id;
      ctx.assert(
        (await ctx.state.b.page.locator(`[data-command-id="${id}"]`).count()) === 1,
        'terminal_exact_operation',
      );
      await absent(
        ctx,
        ctx.state.b.page.locator(
          '.workspace-terminal input, .workspace-terminal textarea, .workspace-terminal [contenteditable="true"]',
        ),
        'terminal_not_interactive_shell',
      );
      await ctx.observe('terminal_receipt', async () => ({
        agent_sha256: hash(ctx.state.f.agentId),
        operation_sha256: hash(id),
        exit_code: ctx.state.operation.exit_code,
      }));
    },
  ]),
];
