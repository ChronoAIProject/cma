import { agentPath, button, define, go, peer, readAgent, ready, shown } from './helpers.mjs';
import {
  agentSetup,
  command,
  completed,
  effectEntries,
  interrupt,
  interrupted,
  running,
  send,
  steering,
  turn,
  verifyTurnPeer,
} from './turn-helpers.mjs';

const open = define('agent.open', 'agent', [
  async (ctx, s) => {
    await agentSetup(ctx, s, 'agent-public-ready');
    await go(s.page, '/agent-manager');
    s.finishOpen = ctx.startAction('open_to_usable');
    await s.page.locator(`a[href="/agents/${s.data.agentId}"]:visible`).click();
    ctx.assert(
      new URL(s.page.url()).pathname === agentPath(s.data.agentId),
      'navigation_opens_exact_agent',
    );
  },
  async (ctx, s) => {
    await ready(ctx, s);
    await shown(
      ctx,
      s.page.locator('.agent-content').filter({ hasText: s.data.historyText }),
      'authorized_history_visible',
    );
    s.finishOpen();
  },
  async (ctx, s) => {
    await go(s.page, '/docs');
    await peer(ctx, s, agentPath(s.data.agentId));
    await ready(ctx, s);
  },
  async (ctx, s) => {
    await readAgent(ctx, s);
    await shown(
      ctx,
      s.page.locator('.agent-content').filter({ hasText: s.data.historyText }),
      'history_same_after_peer_open',
    );
    // Actual auth/connection request receipts are retained by the parent adapter.
    await ctx.observe('open_connection_receipts', () => s.browser.evidence(s.page));
  },
]);
const sendCase = define('agent.send', 'agent', [
  async (ctx, s) => {
    await agentSetup(ctx, s);
    await s.page.getByLabel('Message', { exact: true }).fill(s.data.message);
    ctx.assert(
      (await s.page.getByLabel('Message', { exact: true }).inputValue()) === s.data.message,
      'message_draft_exact',
    );
  },
  async (ctx, s) => {
    s.receipt = await send(ctx, s, s.data.message);
    ctx.assert(s.receipt.input === s.data.message, 'submitted_message_exact');
  },
  async (ctx, s) => {
    await shown(ctx, s.page.locator('.agent-content[data-streaming]'), 'streaming_output_visible');
    await shown(
      ctx,
      s.page.getByText(s.data.toolProgressText, { exact: true }),
      'actual_tool_progress_visible',
    );
    await s.data.runtime.releaseTurn();
    s.terminal = await completed(ctx, s, s.data.message, s.data.expectedReply);
  },
  async (ctx, s) => {
    await verifyTurnPeer(ctx, s, s.receipt, s.data.message, 'completed');
    await completed(ctx, s, s.data.message, s.data.expectedReply);
  },
]);
const stopResponse = define('agent.interrupt', 'agent', [
  async (ctx, s) => {
    await agentSetup(ctx, s);
    await running(ctx, s);
  },
  async (ctx, s) => {
    await interrupt(ctx, s);
  },
  async (ctx, s) => {
    s.terminal = await interrupted(ctx, s);
  },
  async (ctx, s) => {
    await verifyTurnPeer(ctx, s, s.active, s.data.longMessage, 'interrupted');
    ctx.assert(s.interruptRequest.key?.length > 0, 'interrupt_has_stable_identity');
    const effects = effectEntries(ctx, await s.data.runtime.effects(), s);
    ctx.assert(
      effects.filter(
        (effect) => effect.type === 'cancel' && effect.turnSequence === s.terminal.turn_sequence,
      ).length === 1,
      'interrupt_effect_once',
    );
  },
]);
const second = define('agent.second-send', 'agent', [
  async (ctx, s) => {
    await agentSetup(ctx, s);
    await running(ctx, s);
    await interrupt(ctx, s);
    s.old = await interrupted(ctx, s);
  },
  async (ctx, s) => {
    await s.page.getByLabel('Message', { exact: true }).fill(s.data.secondMessage);
    ctx.assert(s.data.secondMessage !== s.data.longMessage, 'second_message_distinct');
  },
  async (ctx, s) => {
    s.receipt = await send(ctx, s, s.data.secondMessage);
    s.next = await completed(ctx, s, s.data.secondMessage, s.data.secondReply);
  },
  async (ctx, s) => {
    await verifyTurnPeer(ctx, s, s.receipt, s.data.secondMessage, 'completed');
    const old = await turn(ctx, s, s.data.longMessage, 'interrupted');
    ctx.assert(
      old.turn_sequence === s.old.turn_sequence && s.next.turn_sequence > old.turn_sequence,
      'second_turn_follows_cancelled_turn',
    );
    const effects = effectEntries(ctx, await s.data.runtime.effects(), s);
    ctx.assert(
      effects.filter(
        (effect) => effect.type === 'start' && effect.turnSequence === old.turn_sequence,
      ).length === 1,
      'cancelled_turn_did_not_restart',
    );
  },
]);
const withMessage = define('agent.interrupt-with-message', 'agent', [
  async (ctx, s) => {
    await agentSetup(ctx, s);
    await running(ctx, s);
    await steering(ctx, s, 'Interrupt');
  },
  async (ctx, s) => {
    // The composite interrupt endpoint owns replacement admission; the case does not create another response.
    await s.page.getByLabel('Message', { exact: true }).fill(s.data.secondMessage);
    const finishReplacement = ctx.startAction('replacement_to_acknowledgement');
    const pending = s.page.waitForResponse(
      (response) =>
        response.request().method() === 'POST' &&
        new URL(response.url()).pathname.endsWith('/' + s.active.responseId + '/interrupt'),
    );
    const [response] = await Promise.all([
      pending,
      button(s.page, 'Interrupt with message').click(),
    ]);
    ctx.assert([200, 202].includes(response.status()), 'interrupt_replacement_admitted');
    s.replacement = await response.json();
    ctx.assert(
      s.replacement.target.response_id === s.active.responseId &&
        s.replacement.response.receipt.agent_id === s.data.agentId &&
        s.replacement.response.receipt.response_id !== s.active.responseId,
      'replacement_bound_to_original_turn',
    );
    finishReplacement();
    await ctx.observe('interrupt_with_message_receipt', async () => ({
      body: s.replacement,
      request_id: response.headers()['x-request-id'],
    }));
  },
  async (ctx, s) => {
    s.old = await turn(ctx, s, s.data.longMessage, 'interrupted');
    s.next = await completed(ctx, s, s.data.secondMessage, s.data.secondReply);
  },
  async (ctx, s) => {
    await peer(ctx, s);
    await readAgent(ctx, s);
    await turn(ctx, s, s.data.secondMessage, 'completed');
    const effects = effectEntries(ctx, await s.data.runtime.effects(), s);
    const oldEnd = effects.findIndex(
      (effect) => effect.type === 'terminal' && effect.turnSequence === s.old.turn_sequence,
    );
    const newStart = effects.findIndex(
      (effect) => effect.type === 'start' && effect.turnSequence === s.next.turn_sequence,
    );
    ctx.assert(oldEnd >= 0 && newStart > oldEnd, 'old_turn_terminal_before_replacement');
    ctx.assert(
      effects.filter((effect) => effect.type === 'start' && effect.input === s.data.secondMessage)
        .length === 1,
      'replacement_effect_once',
    );
    const receipt = s.replacement.response.receipt;
    const status = await command(ctx, s, {
      responseId: receipt.response_id,
      commandId: receipt.command_id,
    });
    ctx.assert(
      status.turn_sequence === s.next.turn_sequence,
      'replacement_receipt_matches_next_turn',
    );
  },
]);
const recovery = define('agent.pending-recovery', 'agent', [
  async (ctx, s) => {
    await agentSetup(ctx, s, 'agent-pending-recovery');
    await s.data.faults.dropObservationAfterCommand();
    ctx.defer('restore_command_observation', () => s.data.faults.restore());
    await s.page.getByLabel('Message', { exact: true }).fill(s.data.message);
    await button(s.page, 'Send message').click();
    s.cut = await s.data.faults.cutReceipt();
    ctx.assert(
      s.cut.accepted && s.cut.commandId && s.cut.responseId && s.cut.key,
      'fault_after_command_admission',
    );
    await shown(ctx, button(s.page, 'Recover pending request'), 'pending_request_explicit');
  },
  async (ctx, s) => {
    await s.data.faults.restore();
    await peer(ctx, s, agentPath(s.data.agentId));
    await shown(
      ctx,
      button(s.page, 'Recover pending request'),
      'retained_pending_request_reopened',
    );
    const sendButton = button(s.page, 'Send message');
    ctx.assert(
      !(await sendButton.isVisible()) || (await sendButton.isDisabled()),
      'new_command_blocked_until_recovery',
    );
  },
  async (ctx, s) => {
    await button(s.page, 'Recover pending request').click();
    await s.data.runtime.releaseTurn();
    await completed(ctx, s, s.data.message, s.data.expectedReply);
    const state = await command(ctx, s, s.cut);
    ctx.assert(state.status === 'completed', 'original_pending_command_completed');
  },
  async (ctx, s) => {
    await verifyTurnPeer(ctx, s, s.cut, s.data.message, 'completed');
    const effects = effectEntries(ctx, await s.data.runtime.effects(), s);
    ctx.assert(
      effects.filter((effect) => effect.type === 'start' && effect.input === s.data.message)
        .length === 1,
      'unknown_effect_not_replayed',
    );
  },
]);
export const cases = [open, sendCase, stopResponse, second, withMessage, recovery];
