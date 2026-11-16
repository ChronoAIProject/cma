import {define, acquire, invoke, egRequest, peer, eventJoin, retainTurn, delivered, digest} from './helpers.mjs';
import {relayVerificationCase} from './relay-verification.mjs';

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

function relayJourney(number, platform, followup) {
  return define(number, 'eg', ['eg'], async ctx => {
    let lease, configuration;
    const turns = [];
    await ctx.step('step_1', async () => {
      lease = await acquire(ctx, `relay_${platform}`);
      const capabilities = await egRequest(ctx, lease, {path: '/v1/channel-capabilities'});
      const advertised = capabilities.relay_authoring?.platforms?.find(p => p.id === platform);
      ctx.assert(advertised !== undefined, 'relay_platform_advertised');
      configuration = await invoke(lease, 'configuration');
      ctx.assert(configuration.platform === platform && configuration.trustMode === 'nyxid_relay',
        'relay_configuration_exact_platform');
      ctx.assert(configuration.issuer === lease.expected.issuer
        && configuration.keyId === lease.expected.keyId
        && configuration.routeId === lease.expected.routeId, 'relay_exact_issuer_key_route');
      ctx.assert(configuration.callbackPath === `/v1/callbacks/${lease.channelId}`,
        'relay_callback_channel');
      ctx.assert(configuration.verificationMode === lease.expected.verificationMode,
        'relay_verification_mode');
      ctx.assert(advertised.verification_modes.some(mode => mode.id === configuration.verificationMode
        && mode.writes_enabled === true), 'relay_exact_mode_advertised');
    });
    await ctx.step('step_2', async () => {
      for (let n = 1; n <= (followup ? 3 : 1); n++) {
        const first = await send(ctx, lease, {platform, trustMode: 'nyxid_relay', replica: n % 2 ? ctx.am : peer(ctx.am),
          sender: 'allowed_sender', conversation: `numbered_${n}`, text: `remember synthetic number ${n}`});
        turns.push(first);
        if (followup) {
          const next = await send(ctx, lease, {platform, trustMode: 'nyxid_relay', replica: n % 2 ? peer(ctx.am) : ctx.am,
            sender: 'allowed_sender', conversation: `numbered_${n}`, text: 'repeat the previous synthetic number'});
          ctx.assert(next.event.conversationId === first.event.conversationId
            && next.event.agentId === first.event.agentId, 'relay_followup_same_context');
          ctx.assert(next.event.responseId !== first.event.responseId, 'relay_followup_distinct_response');
          ctx.assert(next.receipt.contextEventIds.includes(first.event.eventId), 'relay_followup_observed_context');
          turns.push(next);
        }
      }
      ctx.assert(turns.length === (followup ? 6 : 1), 'relay_completed_turn_count');
    });
    if (followup) await ctx.step('step_3', async () => {
      for (const turn of turns) eventJoin(ctx, turn.event, turn.evidence);
      ctx.assert(new Set(turns.map(turn => turn.evidence.servingAm)).size === 2, 'relay_both_am_ingresses');
      ctx.assert(new Set(turns.map(turn => turn.event.egInstance)).size === 2, 'relay_both_eg_ingresses');
    });
    await ctx.step('expected_result', async () => {
      const final = await invoke(lease, 'drain', {eventIds: turns.map(t => t.event.eventId), timeoutMs: 90000});
      ctx.assert(final.pending === 0 && final.unknown === 0 && final.lateEffects === 0, 'relay_terminal_finality');
      ctx.assert(final.turns === turns.length && final.deliveries === turns.length, 'relay_no_duplicate_effects');
      for (const turn of turns) delivered(ctx, turn.event, turn.receipt);
    });
  });
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
