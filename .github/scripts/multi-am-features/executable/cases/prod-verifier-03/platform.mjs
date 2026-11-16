import {define, acquire, invoke, egRequest, peer, retainTurn, delivered, digest} from './helpers.mjs';
import {relayVerificationCase} from './relay-verification.mjs';
import {relayJourney} from './relay-journey.mjs';

export async function send(ctx, lease, input) {
  const event = await invoke(lease, 'platformSend', input);
  ctx.assert(event.platform === input.platform && event.trustMode === input.trustMode,
    'platform_event_trust_mode');
  const evidence = await invoke(lease, 'waitForTurn', {eventId: event.eventId, timeoutMs: 90000});
  await retainTurn(ctx, event, evidence);
  const receipt = await invoke(lease, 'waitForDelivery', {eventId: event.eventId, timeoutMs: 90000});
  delivered(ctx, event, receipt);
  await ctx.observe('platform_event_receipt', async () => ({
    event_sha256: digest(event.eventId), response_sha256: digest(event.responseId),
    conversation_sha256: digest(event.conversationId), serving_am: evidence.servingAm,
    owning_am: evidence.owningAm, epoch: evidence.epoch, platform: input.platform,
    trust_mode: input.trustMode, turn_count: evidence.turnCount, effect_count: evidence.effectCount,
    delivery_count: receipt.count, delivery_sha256: digest(receipt),
  }));
  if (['EG-032', 'EG-033', 'EG-034'].includes(ctx.row.id)) {
    await ctx.observe('external_platform_proof', () => invoke(lease, 'externalPlatformEvidence', {
      eventId: event.eventId, conversationId: event.conversationId, responseId: event.responseId,
      platform: input.platform,
    }));
  }
  return {event, evidence, receipt};
}

export const platformCases = [
  relayJourney(32, 'telegram', true), relayJourney(33, 'discord', true), relayJourney(34, 'whatsapp', true),
  relayJourney(36, 'lark', false), relayJourney(37, 'feishu', false), relayJourney(38, 'slack', false),
  relayVerificationCase,
  define(56, 'eg', ['eg'], async ctx => {
    let lease, event, turn;
    await ctx.step('step_1', async () => {
      lease = await acquire(ctx, 'durable_custom_event');
      const config = await invoke(lease, 'configuration');
      ctx.assert(config.durableCustomExecution === true && config.partition === lease.expected.partition,
        'custom_durability_partition');
      event = await invoke(lease, 'platformSend', {platform: 'custom', replica: ctx.am,
        conversation: 'durable', text: 'retain original response', interruptObservation: true});
      ctx.assert(event.status === 'accepted' && event.eventId && event.conversationId, 'custom_event_admitted');
    });
    await ctx.step('step_2', async () => {
      turn = await invoke(lease, 'recoverResponse', {eventId: event.eventId, replica: peer(ctx.am), timeoutMs: 90000});
      await retainTurn(ctx, event, turn);
      ctx.assert(turn.recoveryMode === 'read_existing' && turn.resubmissions === 0, 'custom_recovery_no_new_turn');
      const refused = await invoke(lease, 'platformSend', {platform: 'custom', replica: peer(ctx.am),
        variant: 'unsupported_durability', text: 'must refuse'});
      ctx.assert(refused.outcome === 'refused' && refused.turnCount === 0, 'custom_unsupported_durability_refused');
    });
    await ctx.step('expected_result', async () => {
      delivered(ctx, event, await invoke(lease, 'waitForDelivery', {eventId: event.eventId, timeoutMs: 90000}));
      const final = await invoke(lease, 'drain', {eventIds: [event.eventId], timeoutMs: 90000});
      ctx.assert(final.turns === 1 && final.deliveries === 1 && final.pending === 0 && final.unknown === 0,
        'custom_one_durable_response');
      await ctx.observe('custom_recovery_receipt', async () => ({response_sha256: digest(turn.responseId),
        event_sha256: digest(event.eventId), serving_am: turn.servingAm, owning_am: turn.owningAm,
        epoch: turn.epoch, resubmissions: turn.resubmissions}));
    });
  }),
];
