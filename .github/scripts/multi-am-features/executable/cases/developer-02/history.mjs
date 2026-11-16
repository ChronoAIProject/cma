import {readFile,stat} from 'node:fs/promises';
import {define,enter,api,button,visible,contains,enc,hash,peerReload,need} from './helpers.mjs';
import {completeExport,retainedEntries} from './history-proof.mjs';

export async function transcript(ctx) {
  const agentId = ctx.state.f.agentId, turns = [], seen = new Set();
  let next, through, pages = 0;
  do {
    const query = new URLSearchParams({...next,limit:'2'});
    const r = await api(ctx,`/api/v1/agents/${enc(agentId)}/transcript?${query}`,{agentId});
    const p = r.body;
    ctx.assert(p.agent_id === agentId && Array.isArray(p.turns)
      && (through === undefined || p.through_seq === through), 'transcript_snapshot_identity');
    through = p.through_seq;
    for (const turn of p.turns) {
      let current = turns.at(-1);
      ctx.assert(!current || current.turn_sequence <= turn.turn_sequence, 'transcript_turn_order');
      if (current?.turn_sequence === turn.turn_sequence) {
        ctx.assert(current.started_at === turn.started_at && JSON.stringify(current.outcome) === JSON.stringify(turn.outcome),
          'transcript_continuation_metadata');
      } else {current = {...turn,messages:[]}; turns.push(current);}
      for (const message of turn.messages) {
        const id = turn.turn_sequence + ':' + message.sequence;
        ctx.assert(!seen.has(id) && message.sequence <= through, 'transcript_message_unique');
        seen.add(id); current.messages.push(message);
      }
    }
    const previous = next?.after ?? -1; next = p.next; pages++;
    ctx.assert(pages <= 256 && (!next || (next.after > previous && next.after < through
      && next.through === through && next.limit === 2)), 'transcript_cursor_progress');
  } while (next);
  return {agent_id:agentId, through_seq:through, turns, pages};
}
export async function shown(ctx,t) {
  for (const turn of t.turns) for (const m of turn.messages) {
    if (!['assistant','user'].includes(m.role)) continue;
    const entry = ctx.state.b.page.locator(`[data-message-key="message:event:${m.sequence}"]`);
    await visible(ctx,entry,'historical_message_visible');
    await contains(ctx,entry,m.content,'historical_message_content');
    ctx.assert(await entry.count() === 1,'historical_message_unique');
  }
}
async function exportClick(ctx) {
  await button(ctx.state.b.page,'More Agent actions').click();
  await ctx.state.b.page.getByRole('menuitem',{name:'Export history',exact:true}).click();
}
export const cases = [
  define('agent.history-export',[
    async ctx => {
      const {page} = await enter(ctx,'agent-history'); ctx.state.before = await transcript(ctx);
      const pending = page.waitForEvent('download').then(d => ({d}), () => ({error:true}));
      await exportClick(ctx); const {d} = await pending;
      ctx.assert(Boolean(d),'history_download_received');
      ctx.defer('remove_history_download',() => d.delete()); ctx.state.download = d;
      ctx.assert(d.suggestedFilename() === `cma-agent-${ctx.state.f.agentId}-history.jsonl`,'history_filename');
    },
    async ctx => {
      const path = await ctx.state.download.path(); ctx.assert(Boolean(path),'private_download_path');
      const info = await stat(path); ctx.assert(info.size > 0 && info.size <= 16*1024*1024,'bounded_export');
      ctx.state.raw = await readFile(path,'utf8');
      await ctx.observe('history_export_digest',async () => ({sha256:hash(ctx.state.raw),bytes:info.size}));
    },
    async ctx => {
      await peerReload(ctx); const current = await transcript(ctx);
      ctx.assert(completeExport(ctx.state.raw,current),'complete_export_projection');
      ctx.assert(retainedEntries(ctx.state.before,current),'export_history_preserved'); await shown(ctx,current);
    },
    async ctx => {
      const f = ctx.state.f;
      need(f.faults?.failNextRead,'history_fault_control_missing');
      const fault = await f.faults.failNextRead({path:`/api/v1/agents/${enc(f.agentId)}/history`,status:503});
      ctx.defer('restore_export_fault',() => fault.restore());
      const downloads = []; const listener = d => downloads.push(d); ctx.state.b.page.on('download',listener);
      ctx.defer('dispose_unexpected_downloads',async () => {
        ctx.state.b.page.off('download',listener); for (const d of downloads) await d.delete();
      });
      await exportClick(ctx);
      await visible(ctx,ctx.state.b.page.getByRole('alert').filter({hasText:'503'}),'export_error_visible');
      ctx.assert(downloads.length === 0,'failed_export_has_no_download');
    },
  ],{scope:'agent'}),
  define('agent.history',[
    async ctx => {
      await enter(ctx,'agent-history'); ctx.state.before = await transcript(ctx);
      ctx.assert(ctx.state.before.turns.some(t => t.outcome?.type === 'completed')
        && ctx.state.before.turns.some(t => t.outcome?.type === 'interrupted'),'terminal_history_variants');
      await shown(ctx,ctx.state.before);
    },
    async ctx => {
      const {page} = ctx.state.b, f = ctx.state.f;
      need(f.toolOutput && f.planTitle,'history_tool_plan_fixture_missing');
      for (const node of await page.locator('.tool-calls__toggle[aria-expanded="false"]').all()) await node.click();
      const tool = page.locator('.tool-call').filter({hasText:f.toolTitle});
      await tool.locator('button[aria-expanded="false"]').first().click();
      await contains(ctx,tool.locator('pre[aria-label="Recorded output"]'),f.toolOutput,'real_tool_detail');
      const plan = page.getByRole('region',{name:'Plan',exact:true}).filter({hasText:f.planSummary});
      const expand = plan.locator('button[aria-expanded="false"]'); if (await expand.count()) await expand.click();
      await contains(ctx,plan,f.planTitle,'real_plan_detail');
      ctx.assert(ctx.state.before.pages > 1,'history_pagination_exercised');
    },
    async ctx => {
      const f = ctx.state.f;
      need(f.faults?.holdNextTranscript && f.provider?.releaseNextEvent,'history_overlap_controls_missing');
      const held = await f.faults.holdNextTranscript({agentId:f.agentId});
      ctx.defer('release_history_hold',() => held.restore()); await peerReload(ctx); await held.waitCaptured();
      const event = await f.provider.releaseNextEvent({agentId:f.agentId}); ctx.state.event = event;
      await contains(ctx,ctx.state.b.page.locator('main'),event.content,'live_event_before_history');
      await held.release(); await shown(ctx,await transcript(ctx));
    },
    async ctx => {
      const current = await transcript(ctx);
      ctx.assert(retainedEntries(ctx.state.before,current),'history_no_loss_or_duplicate');
      ctx.assert(current.turns.some(t => t.messages.some(m => m.sequence === ctx.state.event.sequence
        && m.content === ctx.state.event.content)),'live_event_survives_stale_snapshot');
      const fault = await ctx.state.f.faults.failNextTranscript({agentId:ctx.state.f.agentId,status:503});
      ctx.defer('restore_history_fault',() => fault.restore()); await ctx.state.b.page.reload();
      await visible(ctx,ctx.state.b.page.getByText('Earlier activity could not be loaded. Retrying…',{exact:true}),
        'history_unavailable_explicit');
    },
  ],{scope:'agent'}),
];
