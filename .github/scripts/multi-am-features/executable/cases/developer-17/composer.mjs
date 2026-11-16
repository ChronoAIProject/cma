import { button, define, digest, peer, readAgent, shown, transcript } from './helpers.mjs';
import { agentSetup, completed, sendAction, verifyTurnPeer } from './turn-helpers.mjs';

const commands = define('agent.commands', 'agent', [
  async (ctx, s) => {
    await agentSetup(ctx, s, 'agent-commands');
    await s.page.getByLabel('Message', { exact: true }).fill('/');
    await shown(
      ctx,
      s.page.getByRole('listbox', { name: 'Agent commands' }),
      'runtime_command_menu_visible',
    );
  },
  async (ctx, s) => {
    await s.page
      .getByRole('listbox', { name: 'Agent commands' })
      .getByRole('option', { name: s.data.command.label, exact: true })
      .click();
    ctx.assert(
      (await s.page.getByLabel('Message', { exact: true }).inputValue()).trim() ===
        s.data.command.text,
      'advertised_command_selected',
    );
  },
  async (ctx, s) => {
    s.receipt = await sendAction(ctx, s, () => button(s.page, 'Send message').click());
    await completed(ctx, s, s.data.command.text, s.data.command.expectedReply);
  },
  async (ctx, s) => {
    const before = s.writes.length;
    await s.page.getByLabel('Message', { exact: true }).fill(s.data.unsupportedCommand);
    await button(s.page, 'Send message').click();
    await shown(
      ctx,
      s.page
        .getByRole('alert')
        .filter({ hasText: 'This command is not available for this Agent.' }),
      'unsupported_command_refusal',
    );
    ctx.assert(
      (await s.page.getByLabel('Message', { exact: true }).inputValue()) ===
        s.data.unsupportedCommand,
      'unsupported_command_draft_retained',
    );
    ctx.assert(s.writes.length === before, 'unsupported_command_not_submitted');
    await s.page.getByLabel('Message', { exact: true }).fill(s.data.command.text);
    await s.page.getByLabel('Choose attachments', { exact: true }).setInputFiles(s.data.attachment);
    await shown(
      ctx,
      s.page
        .getByRole('list', { name: 'Attachments' })
        .getByRole('status')
        .filter({ hasText: /^Ready$/ }),
      'command_attachment_upload_ready',
    );
    await button(s.page, 'Send message').click();
    await shown(
      ctx,
      s.page.getByRole('alert').filter({
        hasText:
          'Send attachments with a message or skill invocation, separately from a session command.',
      }),
      'command_attachment_combination_refused',
    );
    ctx.assert(s.writes.length === before, 'command_attachment_not_submitted');
  },
  async (ctx, s) => {
    await verifyTurnPeer(ctx, s, s.receipt, s.data.command.text, 'completed');
  },
]);
const suggestions = define('agent.suggestions', 'agent', [
  async (ctx, s) => {
    await agentSetup(ctx, s, 'agent-suggestions');
    await shown(
      ctx,
      s.page.getByRole('region', { name: 'Next you can ask' }),
      'suggestions_visible',
    );
    await shown(
      ctx,
      s.page
        .getByRole('region', { name: 'Next you can ask' })
        .getByText(s.data.suggestion, { exact: true }),
      'actual_suggestion_text',
    );
  },
  async (ctx, s) => {
    await s.page.getByLabel('Message', { exact: true }).fill(s.data.draft);
    await button(s.page, 'Edit suggestion 1 before sending').click();
    await shown(
      ctx,
      s.page.getByRole('dialog', { name: 'Keep your draft?' }),
      'draft_conflict_confirmation',
    );
  },
  async (ctx, s) => {
    await button(s.page.getByRole('dialog', { name: 'Keep your draft?' }), 'Cancel').click();
    ctx.assert(
      (await s.page.getByLabel('Message', { exact: true }).inputValue()) === s.data.draft,
      'suggestion_cancel_keeps_draft',
    );
    await button(s.page, 'Edit suggestion 1 before sending').click();
    await button(s.page, 'Replace text').click();
    ctx.assert(
      (await s.page.getByLabel('Message', { exact: true }).inputValue()) === s.data.suggestion,
      'suggestion_replaces_explicitly',
    );
    await button(s.page, 'Undo replacement').click();
    ctx.assert(
      (await s.page.getByLabel('Message', { exact: true }).inputValue()) === s.data.draft,
      'suggestion_undo_restores_draft',
    );
    await button(s.page, 'Edit suggestion 1 before sending').click();
    await button(s.page, 'Append to draft').click();
    s.chosen = s.data.draft + '\n' + s.data.suggestion;
    ctx.assert(
      (await s.page.getByLabel('Message', { exact: true }).inputValue()) === s.chosen,
      'suggestion_append_preserves_draft',
    );
    ctx.assert(s.writes.length === 0, 'suggestion_edit_not_implicit_send');
  },
  async (ctx, s) => {
    s.receipt = await sendAction(ctx, s, () => button(s.page, 'Send message').click());
    ctx.assert(s.receipt.input === s.chosen, 'only_explicit_chosen_text_sent');
    await s.data.runtime.releaseTurn();
  },
  async (ctx, s) => {
    await verifyTurnPeer(ctx, s, s.receipt, s.chosen, 'completed');
  },
]);
const quick = define('agent.quick-messages', 'agent', [
  async (ctx, s) => {
    await agentSetup(ctx, s, 'agent-quick-messages');
    await shown(
      ctx,
      s.page.getByRole('group', { name: 'Trigger quick messages' }),
      'trigger_quick_messages_visible',
    );
    const source = await ctx.request({
      method: 'GET',
      path: '/api/v1/agents/' + encodeURIComponent(s.data.agentId) + '/source',
      agentId: s.data.agentId,
    });
    ctx.assert(
      source.body.origin.type === 'trigger' && source.body.agent_id === s.data.agentId,
      'actual_trigger_agent_origin',
    );
    await s.page.getByLabel('Message', { exact: true }).fill(s.data.draft);
  },
  async (ctx, s) => {
    await button(
      s.page.getByRole('group', { name: 'Trigger quick messages' }),
      s.data.quickMessage.title,
    ).hover();
    await shown(
      ctx,
      s.page.getByRole('tooltip').filter({ hasText: s.data.quickMessage.description }),
      'quick_message_preview_visible',
    );
    ctx.assert(s.writes.length === 0, 'quick_message_preview_not_sent');
  },
  async (ctx, s) => {
    s.receipt = await sendAction(ctx, s, () =>
      button(
        s.page.getByRole('group', { name: 'Trigger quick messages' }),
        s.data.quickMessage.title,
      ).click(),
    );
    ctx.assert(
      s.receipt.input === s.data.quickMessage.message,
      'quick_message_exact_predefined_text',
    );
  },
  async (ctx, s) => {
    await completed(ctx, s, s.data.quickMessage.message, s.data.expectedReply);
    ctx.assert(
      (await s.page.getByLabel('Message', { exact: true }).inputValue()) === s.data.draft,
      'quick_message_keeps_existing_draft',
    );
  },
  async (ctx, s) => {
    await verifyTurnPeer(ctx, s, s.receipt, s.data.quickMessage.message, 'completed');
  },
]);
const copy = define('agent.copy-response', 'agent', [
  async (ctx, s) => {
    await agentSetup(ctx, s, 'agent-copy');
    s.message = s.page
      .locator('[data-message-key]')
      .filter({ has: s.page.locator('.agent-content').filter({ hasText: s.data.codeText }) });
    ctx.assert((await s.message.count()) === 1, 'copy_exact_message_selected');
    await shown(
      ctx,
      s.message.getByRole('button', { name: 'Copy content', exact: true }),
      'copy_content_available',
    );
  },
  async (ctx, s) => {
    await button(s.message, 'Copy content').click();
    await shown(
      ctx,
      s.message.getByRole('status').filter({ hasText: /^Copied$/ }),
      'copy_content_feedback',
    );
    s.content = await s.page.evaluate(() => navigator.clipboard.readText());
    ctx.assert(s.content === s.data.messageText, 'copied_exact_response_content');
    await button(s.message, 'Copy code').click();
    await shown(
      ctx,
      s.message.getByRole('status').filter({ hasText: /^Code copied$/ }),
      'copy_code_feedback',
    );
    s.code = await s.page.evaluate(() => navigator.clipboard.readText());
    ctx.assert(s.code === s.data.codeText, 'copied_exact_code_only');
  },
  async (ctx, s) => {
    ctx.assert(
      (await s.message.locator('.code-block pre').textContent()).replace(/\n$/, '') === s.code,
      'clipboard_matches_displayed_code',
    );
    const turns = await transcript(ctx, s);
    ctx.assert(
      turns.some((row) => row.messages.some((message) => message.content === s.content)),
      'copied_response_matches_durable_message',
    );
  },
  async (ctx, s) => {
    await peer(ctx, s);
    await readAgent(ctx, s);
    await ctx.observe('copy_content_hashes', async () => ({
      response: digest(s.content),
      code: digest(s.code),
    }));
  },
]);
export const cases = [commands, suggestions, quick, copy];
