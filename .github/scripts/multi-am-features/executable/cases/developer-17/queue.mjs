import { absent, button, define, peer, readAgent, shown, transcript, uiWrite } from './helpers.mjs';
import {
  agentSetup,
  command,
  effectEntries,
  responsePath,
  running,
  send,
  steering,
  turn,
  until,
} from './turn-helpers.mjs';

const queuedPath = (s) => '/api/v2/agents/' + encodeURIComponent(s.data.agentId) + '/queued-inputs';
async function queued(ctx, s) {
  const page = await ctx.request({
    method: 'GET',
    am: s.readAm,
    path: queuedPath(s),
    agentId: s.data.agentId,
  });
  ctx.assert(
    page.body.agent_id === s.data.agentId && Array.isArray(page.body.data),
    'queued_page_agent_identity',
  );
  ctx.assert(page.body.next_after === null, 'queued_fixture_fits_one_page');
  return page.body.data;
}
async function queueTwo(ctx, s) {
  await agentSetup(ctx, s, 'agent-queue');
  await running(ctx, s);
  await steering(ctx, s, 'Queue');
  s.queued = [];
  for (const message of s.data.queueMessages)
    s.queued.push(await send(ctx, s, message, 'Queue message'));
  ctx.assert(
    s.queued.length === 2 && s.queued[0].responseId !== s.queued[1].responseId,
    'two_distinct_queue_receipts',
  );
  s.before = await queued(ctx, s);
  ctx.assert(
    JSON.stringify(s.before.map((item) => item.text)) === JSON.stringify(s.data.queueMessages),
    'queue_observed_order',
  );
}
async function assertQueueEffects(ctx, s, expected) {
  const journal = effectEntries(ctx, await s.data.runtime.effects(), s);
  const starts = journal.filter((event) => event.type === 'start');
  ctx.assert(
    JSON.stringify(starts.map((event) => event.input)) ===
      JSON.stringify([s.data.longMessage, ...expected]),
    'queue_execution_order_exactly_once',
  );
  await ctx.observe('queue_execution_journal', async () => journal);
}
const queue = define('agent.queue', 'agent', [
  async (ctx, s) => {
    await agentSetup(ctx, s, 'agent-queue');
    await running(ctx, s);
    await steering(ctx, s, 'Queue');
  },
  async (ctx, s) => {
    s.queued = [];
    for (const message of s.data.queueMessages)
      s.queued.push(await send(ctx, s, message, 'Queue message'));
    ctx.assert(
      s.queued.length === 2 && s.queued[0].responseId !== s.queued[1].responseId,
      'two_unique_queued_messages',
    );
  },
  async (ctx, s) => {
    const rows = await queued(ctx, s);
    ctx.assert(
      JSON.stringify(rows.map((row) => row.text)) === JSON.stringify(s.data.queueMessages),
      'queued_order_visible_durably',
    );
    for (const message of s.data.queueMessages)
      await shown(
        ctx,
        s.page.getByRole('article').filter({ hasText: message }),
        'queued_message_visible',
      );
    await s.data.runtime.releaseTurn();
    for (const message of s.data.queueMessages) await turn(ctx, s, message, 'completed');
  },
  async (ctx, s) => {
    await peer(ctx, s);
    await readAgent(ctx, s);
    ctx.assert((await queued(ctx, s)).length === 0, 'queue_drained');
    await assertQueueEffects(ctx, s, s.data.queueMessages);
  },
]);
const edit = define('agent.queue-edit', 'agent', [
  async (ctx, s) => {
    await queueTwo(ctx, s);
  },
  async (ctx, s) => {
    const item = s.page.getByRole('article', { name: 'Queued message 1', exact: true });
    await button(item, 'Edit queued message content').click();
    const form = item.getByRole('form', { name: 'Edit queued message' });
    await form.getByLabel('Queued message', { exact: true }).fill(s.data.editedMessage);
    await form.getByLabel('Choose attachments', { exact: true }).setInputFiles(s.data.attachment);
    await shown(
      ctx,
      form.getByRole('status').filter({ hasText: /^Ready$/ }),
      'queued_attachment_uploaded',
    );
    ctx.assert(
      (await form.getByLabel('Queued message', { exact: true }).inputValue()) ===
        s.data.editedMessage,
      'queued_edit_draft_exact',
    );
  },
  async (ctx, s) => {
    const path = responsePath(s.data.agentId) + '/' + s.queued[0].responseId + '/queued-input';
    const result = await uiWrite(ctx, s, 'POST', path, () =>
      button(s.page.getByRole('form', { name: 'Edit queued message' }), 'Save').click(),
    );
    const before = s.before[0];
    ctx.assert(
      result.submitted.source_command_id === s.queued[0].commandId &&
        result.submitted.expected_command_revision === before.command_revision,
      'queued_edit_exact_revision',
    );
    ctx.assert(
      result.body.command_revision === before.command_revision + 1,
      'queued_edit_revision_advanced',
    );
    s.editedInput = result.body.text;
    ctx.assert(
      s.editedInput.startsWith(s.data.editedMessage + '\n\nAttached files') &&
        s.editedInput.includes(s.data.attachment.name),
      'queued_edit_text_and_attachment',
    );
    const rows = await queued(ctx, s);
    ctx.assert(
      rows[0].text === s.editedInput && rows[1].text === s.data.queueMessages[1],
      'edit_only_selected_queue_entry',
    );
    await s.data.runtime.releaseTurn();
    await turn(ctx, s, s.editedInput, 'completed');
  },
  async (ctx, s) => {
    await peer(ctx, s);
    await turn(ctx, s, s.data.queueMessages[1], 'completed');
    await assertQueueEffects(ctx, s, [s.editedInput, s.data.queueMessages[1]]);
    await queueRace(ctx, 'edit');
  },
]);
const remove = define('agent.queue-remove', 'agent', [
  async (ctx, s) => {
    await queueTwo(ctx, s);
  },
  async (ctx, s) => {
    const path =
      responsePath(s.data.agentId) + '/' + s.queued[0].responseId + '/queued-cancellation';
    const result = await uiWrite(ctx, s, 'POST', path, () =>
      button(s.page, 'Remove queued message 1').click(),
    );
    ctx.assert(
      result.submitted.source_command_id === s.queued[0].commandId &&
        result.submitted.expected_command_revision === s.before[0].command_revision,
      'queue_cancel_exact_identity_revision',
    );
    ctx.assert(
      result.body.response.receipt.command_id === s.queued[0].commandId &&
        result.body.response.status === 'cancelled',
      'source_queue_cancelled',
    );
  },
  async (ctx, s) => {
    await absent(
      ctx,
      s.page.getByRole('article').filter({ hasText: s.data.queueMessages[0] }),
      'removed_queue_not_visible',
    );
    const state = await command(ctx, s, s.queued[0]);
    ctx.assert(
      state.status === 'cancelled' && state.turn_sequence === null,
      'cancelled_queue_never_dispatched',
    );
    await s.data.runtime.releaseTurn();
    await turn(ctx, s, s.data.queueMessages[1], 'completed');
  },
  async (ctx, s) => {
    await peer(ctx, s);
    await readAgent(ctx, s);
    ctx.assert(
      (await transcript(ctx, s)).every((row) => row.owner_message !== s.data.queueMessages[0]),
      'cancelled_queue_has_no_turn',
    );
    await assertQueueEffects(ctx, s, [s.data.queueMessages[1]]);
    await queueRace(ctx, 'remove');
  },
]);
async function queueRace(ctx, action) {
  const s = {};
  await queueTwo(ctx, s);
  if (action === 'edit') {
    await button(
      s.page.getByRole('article', { name: 'Queued message 1', exact: true }),
      'Edit queued message content',
    ).click();
    await s.page
      .getByRole('form', { name: 'Edit queued message' })
      .getByLabel('Queued message', { exact: true })
      .fill(s.data.editedMessage);
  }
  // Pause only the local browser's event observation. The real worker keeps running.
  await s.data.faults.pauseQueueObservation();
  ctx.defer('restore_queue_observation', () => s.data.faults.restore());
  await s.data.runtime.releaseTurn();
  await until(
    ctx,
    () => command(ctx, s, s.queued[0]),
    (state) => state.turn_sequence !== null,
    'race_target_already_dispatched',
  );
  if (action === 'edit')
    await button(s.page.getByRole('form', { name: 'Edit queued message' }), 'Save').click();
  else await button(s.page, 'Remove queued message 1').click();
  await shown(
    ctx,
    s.page.locator('.queued-messages__error').filter({ hasText: s.data.raceRefusalText[action] }),
    'queue_race_explicit_refusal',
  );
  await s.data.faults.restore();
  const turns = await transcript(ctx, s);
  ctx.assert(
    turns.some((row) => row.owner_message === s.data.queueMessages[0]) &&
      turns.every((row) => row.owner_message !== s.data.editedMessage),
    'queue_race_did_not_overwrite_started_turn',
  );
}
export const cases = [queue, edit, remove];
