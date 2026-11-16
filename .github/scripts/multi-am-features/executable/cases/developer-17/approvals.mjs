import { absent, button, define, peer, readAgent, shown } from './helpers.mjs';
import { agentSetup, command, effectEntries, send, turn } from './turn-helpers.mjs';

function approvalCase(decision) {
  const label = decision[0].toUpperCase() + decision.slice(1);
  return define('agent.approval-' + decision, 'agent', [
    async (ctx, s) => {
      await agentSetup(ctx, s, 'agent-approval');
      s.receipt = await send(ctx, s, s.data.approval.triggerMessage);
      s.card = s.page.getByRole('region', { name: s.data.approval.reason, exact: true });
      await shown(ctx, s.card, 'guarded_approval_visible');
      const heading = s.card.getByRole('heading');
      s.approvalId = (await heading.getAttribute('id')).replace(/^approval-/, '');
      ctx.assert(s.approvalId === s.data.approval.requestId, 'exact_guarded_approval_id');
    },
    async (ctx, s) => {
      const pending = s.page.waitForResponse(
        (response) =>
          response.request().method() === 'POST' &&
          (new URL(response.url()).pathname.endsWith(
            '/approvals/' + encodeURIComponent(s.approvalId),
          ) ||
            new URL(response.url()).pathname.endsWith('/' + s.receipt.responseId + '/controls')),
      );
      const [response] = await Promise.all([pending, button(s.card, label).click()]);
      ctx.assert([200, 202].includes(response.status()), 'approval_admitted');
      s.answer = {
        path: new URL(response.url()).pathname,
        body: response.request().postDataJSON(),
        key: response.request().headers()['idempotency-key'],
      };
      const answer = s.answer.body.command ?? s.answer.body;
      ctx.assert(answer.decision === decision, 'selected_approval_decision');
      if (answer.approval_id)
        ctx.assert(answer.approval_id === s.approvalId, 'control_exact_approval_target');
      await ctx.observe('approval_answer', async () => ({
        ...s.answer,
        request_id: response.headers()['x-request-id'],
      }));
    },
    async (ctx, s) => {
      s.terminal = await turn(
        ctx,
        s,
        s.data.approval.triggerMessage,
        decision === 'abort' ? ['interrupted', 'completed'] : 'completed',
      );
      await absent(ctx, button(s.card, 'Allow'), 'answered_approval_no_active_allow');
      const result = await command(ctx, s, s.receipt);
      ctx.assert(result.turn_sequence === s.terminal.turn_sequence, 'approval_turn_identity');
    },
    async (ctx, s) => {
      await peer(ctx, s);
      await readAgent(ctx, s);
      const before = effectEntries(ctx, await s.data.runtime.effects(), s);
      const requested = before.filter(
        (event) => event.type === 'effect' && event.name === s.data.approval.effect,
      );
      ctx.assert(
        requested.length === (decision === 'allow' ? 1 : 0),
        'approval_effect_matches_decision',
      );
      if (decision === 'abort')
        ctx.assert(
          before.filter(
            (event) => event.type === 'approval_answer' && event.approvalId === s.approvalId,
          ).length === 0,
          'abort_supplies_no_approval_answer',
        );
      const resolutions = before.filter(
        (event) =>
          event.approvalId === s.approvalId &&
          event.type === (decision === 'abort' ? 'approval_cancel' : 'approval_answer'),
      );
      ctx.assert(
        resolutions.length === 1 && (decision === 'abort' || resolutions[0].decision === decision),
        'exact_approval_resolution_once',
      );
      // Start another real guarded request, then replay the prior target with a distinct intent.
      const second = await send(ctx, s, s.data.approval.secondTriggerMessage);
      await shown(
        ctx,
        s.page.getByRole('region', { name: s.data.approval.secondReason, exact: true }),
        'second_guarded_action_waiting',
      );
      const stale = await ctx.request({
        method: 'POST',
        agentId: s.data.agentId,
        path: s.answer.path,
        body: s.answer.body,
        key: 'stale-' + s.receipt.commandId,
        allowed: [200, 202, 409, 410, 422],
      });
      if ([200, 202].includes(stale.status)) {
        // Accepted retained control must still name the old target, never authorize the new one.
        ctx.assert(
          stale.body.target?.response_id === s.receipt.responseId,
          'stale_receipt_retains_old_target',
        );
      } else ctx.assert(stale.status >= 400, 'stale_approval_refused');
      const after = effectEntries(ctx, await s.data.runtime.effects(), s);
      ctx.assert(
        after.filter((event) => event.type === 'effect' && event.name === s.data.approval.effect)
          .length === requested.length,
        'answered_approval_not_reexecuted',
      );
      ctx.assert(
        after.filter(
          (event) => event.type === 'effect' && event.name === s.data.approval.secondEffect,
        ).length === 0,
        'stale_approval_did_not_authorize_new_work',
      );
      await shown(
        ctx,
        s.page
          .getByRole('region', { name: s.data.approval.secondReason, exact: true })
          .getByRole('button', { name: 'Allow', exact: true }),
        'new_approval_still_requires_consent',
      );
      ctx.assert(second.responseId !== s.receipt.responseId, 'approval_requests_distinct');
    },
  ]);
}
export const cases = ['allow', 'deny', 'abort'].map(approvalCase);
