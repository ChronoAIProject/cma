import {
  agentPath,
  browserEvidence,
  button,
  go,
  peer,
  readAgent,
  ready,
  setup,
  shown,
  transcript,
  watchWrites,
} from './helpers.mjs';

export const responsePath = (id) => '/api/v2/agents/' + encodeURIComponent(id) + '/responses';
export async function agentSetup(ctx, s, name = 'agent-ready') {
  await setup(ctx, s, name);
  await go(s.page, agentPath(s.data.agentId));
  await ready(ctx, s);
  s.writes = watchWrites(ctx, s, responsePath(s.data.agentId));
}
export async function send(ctx, s, message, label = 'Send message') {
  await s.page.getByLabel('Message', { exact: true }).fill(message);
  return sendAction(ctx, s, () => button(s.page, label).click());
}
export async function sendAction(ctx, s, action) {
  const finish = ctx.startAction(s.preparingTurn ? 'setup_send_to_acknowledgement' : 'send_to_acknowledgement');
  const started = performance.now();
  const pending = s.page.waitForResponse(
    (response) =>
      response.request().method() === 'POST' &&
      new URL(response.url()).pathname === responsePath(s.data.agentId),
    { timeout: 30000 },
  );
  const [response] = await Promise.all([pending, action()]);
  // V2 create is SSE. The UI deliberately closes it after observing these headers.
  const headers = response.headers();
  const receipt = {
    responseId: headers['x-cma-response-id'],
    commandId: headers['x-cma-command-id'],
    requestId: headers['x-request-id'],
    key: response.request().headers()['idempotency-key'],
    input: response.request().postDataJSON().input,
  };
  ctx.assert(
    response.status() === 200 &&
      typeof receipt.responseId === 'string' &&
      typeof receipt.commandId === 'string' &&
      typeof receipt.key === 'string',
    'sse_admission_receipt',
  );
  finish();
  await ctx.observe('response_admission', async () => receipt);
  await ctx.observe('send_admission_timing', async () => ({
    elapsed_ms: performance.now() - started,
    target_ms: 1000,
    terminal: false,
    response_id: receipt.responseId,
    command_id: receipt.commandId,
  }));
  return receipt;
}
export async function command(ctx, s, receipt) {
  const response = await ctx.request({
    method: 'GET',
    am: s.readAm,
    agentId: s.data.agentId,
    path: '/api/v2/commands/' + encodeURIComponent(receipt.commandId),
  });
  ctx.assert(
    response.body.receipt.agent_id === s.data.agentId &&
      response.body.receipt.command_id === receipt.commandId &&
      response.body.receipt.response_id === receipt.responseId,
    'exact_command_identity',
  );
  return response.body;
}
export async function until(ctx, read, done, code, timeout = 60000) {
  const deadline = performance.now() + timeout;
  for (;;) {
    const value = await read();
    if (done(value)) {
      ctx.assert(true, code);
      return value;
    }
    if (performance.now() >= deadline) {
      ctx.assert(false, code);
    }
    // Observations only: no mutation retry and no poll faster than two seconds.
    await new Promise((resolve) => setTimeout(resolve, 2000));
  }
}
export async function turn(ctx, s, message, outcome) {
  const turns = await until(
    ctx,
    () => transcript(ctx, s),
    (rows) => rows.some((row) => row.owner_message === message && row.outcome.type !== 'active'),
    'turn_reached_terminal',
  );
  const matching = turns.filter((row) => row.owner_message === message);
  ctx.assert(matching.length === 1, 'turn_exactly_once');
  ctx.assert(
    (Array.isArray(outcome) ? outcome : [outcome]).includes(matching[0].outcome.type),
    'expected_terminal_outcome',
  );
  return matching[0];
}
export async function completed(ctx, s, message, reply) {
  const value = await turn(ctx, s, message, 'completed');
  ctx.assert(value.agent_messages.includes(reply), 'durable_reply_matches');
  await shown(
    ctx,
    s.page.locator('.agent-content').filter({ hasText: reply }),
    'completed_reply_visible',
  );
  return value;
}
export async function running(ctx, s) {
  s.preparingTurn = true;
  try {s.active = await send(ctx, s, s.data.longMessage);}
  finally {s.preparingTurn = false;}
  const state = await until(
    ctx,
    () => command(ctx, s, s.active),
    (value) => value.turn_sequence !== null,
    'bounded_turn_started',
  );
  ctx.assert(['in_progress', 'waiting_action'].includes(state.status), 'bounded_turn_active');
  await shown(ctx, button(s.page, 'Stop response'), 'stop_response_available');
}
export async function interrupt(ctx, s) {
  s.finishInterrupt = ctx.startAction('interrupt_to_terminal');
  s.interruptStarted = performance.now();
  const path = responsePath(s.data.agentId) + '/' + encodeURIComponent(s.active.responseId);
  const pending = s.page.waitForResponse(
    (response) =>
      response.request().method() === 'POST' &&
      [path + '/controls', path + '/interrupt'].includes(new URL(response.url()).pathname),
    { timeout: 30000 },
  );
  const [response] = await Promise.all([pending, button(s.page, 'Stop response').click()]);
  ctx.assert([200, 202].includes(response.status()), 'interrupt_admitted');
  s.interruptRequest = {
    path: new URL(response.url()).pathname,
    body: response.request().postDataJSON(),
    key: response.request().headers()['idempotency-key'],
    requestId: response.headers()['x-request-id'],
  };
  await ctx.observe('interrupt_request', async () => s.interruptRequest);
}
export async function interrupted(ctx, s) {
  await shown(
    ctx,
    s.page.locator('.workspace-stopped').getByText(/^Stopped(?: after .+)?$/),
    'interrupt_ui_terminal',
  );
  await ready(ctx, s);
  s.finishInterrupt?.();
  const value = await turn(ctx, s, s.data.longMessage, 'interrupted');
  await ctx.observe('interrupt_confirmed_timing', async () => ({
    elapsed_ms: performance.now() - s.interruptStarted,
    target_ms: 1000,
    terminal: true,
    turn_sequence: value.turn_sequence,
  }));
  return value;
}
export async function steering(ctx, s, label) {
  await button(s.page, /^Steering mode: /).click();
  await s.page
    .getByRole('menu', { name: 'Steering mode' })
    .locator(`[data-id="${label.toLowerCase()}"]`)
    .click();
  await shown(ctx, button(s.page, `Steering mode: ${label}`), 'steering_mode_selected');
}
export async function verifyTurnPeer(ctx, s, receipt, message, outcome) {
  await peer(ctx, s, agentPath(s.data.agentId));
  await readAgent(ctx, s);
  const state = await command(ctx, s, receipt);
  const terminal = await turn(ctx, s, message, outcome);
  ctx.assert(state.turn_sequence === terminal.turn_sequence, 'command_matches_durable_turn');
  const writes = s.writes.filter((write) => write.body.input === message);
  ctx.assert(
    writes.length > 0 && writes.every((write) => write.key === receipt.key),
    'turn_keeps_admission_identity',
  );
  await browserEvidence(ctx, s);
  return terminal;
}
export function effectEntries(ctx, effects, s) {
  ctx.assert(
    Array.isArray(effects) && effects.every((effect) => effect.agentId === s.data.agentId),
    'runtime_journal_exact_agent',
  );
  return effects;
}
