import {define,enter,api,button,visible,contains,absent,enc,hash,peerReload,need,waitFor} from './helpers.mjs';
const panel = ctx => ctx.state.b.page.getByRole('region',{name:'Machine desktop',exact:true});
async function screen(ctx,name='desktop') {
  const b = await enter(ctx,name);
  await button(b.page,'Show Agent computer').click();
  if (name === 'browser') await b.page.getByRole('tab',{name:'Browser',exact:true}).click();
  return b;
}
export async function pixels(ctx,kind) {
  const node = kind === 'desktop' ? ctx.state.b.page.locator('[aria-label="Linux desktop screen"] canvas').first()
    : ctx.state.b.page.getByRole('img',{name:'Agent browser page',exact:true});
  await visible(ctx,node,'display_frame_element');
  await waitFor(()=>node.evaluate(n => n instanceof HTMLImageElement ? n.complete && n.naturalWidth>0 : n.width>0 && n.height>0),'frame_decoded');
  const sample = await node.evaluate((n,marker) => {
    const canvas = n instanceof HTMLCanvasElement ? n : document.createElement('canvas');
    if (n instanceof HTMLImageElement) {canvas.width=n.naturalWidth;canvas.height=n.naturalHeight;canvas.getContext('2d').drawImage(n,0,0);}
    const c=canvas.getContext('2d'), p=c.getImageData(marker.x,marker.y,1,1).data;
    return {width:canvas.width,height:canvas.height,pixel:[...p]};
  },ctx.state.f.frameMarker);
  ctx.assert(sample.width>0 && sample.height>0 && sample.pixel.length===4
    && sample.pixel.every((n,i)=>Math.abs(n-ctx.state.f.frameMarker.rgba[i])<=3),'actual_frame_marker_rendered');
  await ctx.observe('rendered_frame',async()=>({...sample,screenshot_sha256:hash((await node.screenshot()).toString('base64'))}));
}
async function noControl(ctx) {
  await waitFor(()=>panel(ctx).getAttribute('data-controller').then(v=>v!=='user'),'desktop_control_revoked');
  ctx.assert(await panel(ctx).getAttribute('data-controller')!=='user','desktop_no_implicit_control');
}
export const cases = [
  define('machine.view',[
    async ctx=>{await screen(ctx); await visible(ctx,panel(ctx),'desktop_exact_panel');},
    async ctx=>{await pixels(ctx,'desktop'); await contains(ctx,panel(ctx),'Connected','desktop_connected_after_frame');},
    async ctx=>{
      const f=ctx.state.f; need(f.provider?.desktopAction,'desktop_action_control_missing');
      await f.provider.desktopAction({agentId:f.agentId,marker:f.actionMarker});
      await contains(ctx,panel(ctx),'Agent working','desktop_working_status');
      const journal=await f.runtime.actions({agentId:f.agentId});
      ctx.assert(journal.some(e=>e.agent_id===f.agentId && e.marker===f.actionMarker),'desktop_action_exact_agent');
    },
    async ctx=>{await ctx.state.f.provider.finishTurn({agentId:ctx.state.f.agentId}); await peerReload(ctx);
      await button(ctx.state.b.page,'Show Agent computer').click(); await pixels(ctx,'desktop'); await noControl(ctx);},
  ],{scope:'agent'}),
  define('machine.control',[
    async ctx=>{
      const {f}=await screen(ctx); need(f.provider?.beginHeldTurn && f.runtime?.inputs,'desktop_control_fixture_missing');
      await f.provider.beginHeldTurn({agentId:f.agentId}); await pixels(ctx,'desktop');
      await button(panel(ctx),'Control desktop').click();
      await visible(ctx,ctx.state.b.page.getByRole('dialog',{name:'Take control',exact:true}),'takeover_requires_confirmation');
      await noControl(ctx);
    },
    async ctx=>{
      await button(ctx.state.b.page.getByRole('dialog',{name:'Take control'}),'Stop Agent and take control').click();
      await contains(ctx,panel(ctx),'Waiting for the Agent to stop…','takeover_waits_stop');
      await noControl(ctx); await ctx.state.f.provider.releaseInterrupt({agentId:ctx.state.f.agentId});
    },
    async ctx=>{
      await waitFor(()=>panel(ctx).getAttribute('data-controller').then(v=>v==='user'),'desktop_control_granted');
      ctx.assert(await panel(ctx).getAttribute('data-controller')==='user','idle_input_authorized');
      await panel(ctx).locator('canvas').first().click({position:ctx.state.f.harmlessPoint});
      const journal=await ctx.state.f.runtime.inputs({agentId:ctx.state.f.agentId});
      ctx.assert(journal.length>0 && journal.at(-1).after_confirmed_idle===true,'input_after_confirmed_idle');
    },
    async ctx=>{
      await button(panel(ctx),'Return to Agent').click(); await noControl(ctx);
      await ctx.state.b.page.getByRole('textbox',{name:'Message Agent'}).fill(ctx.state.f.nextTask);
      await button(ctx.state.b.page,'Send message').click(); await contains(ctx,panel(ctx),'Agent working','new_task_desktop_working');
    },
    async ctx=>{
      const f=ctx.state.f; await noControl(ctx);
      await f.provider.finishTurn({agentId:f.agentId}); await button(panel(ctx),'Control desktop').click();
      await button(panel(ctx),'Reconnect').click(); await noControl(ctx); await pixels(ctx,'desktop');
      const fault=await f.runtime.disconnect({agentId:f.agentId});ctx.defer('restore_desktop_stream',()=>fault.restore());
      await noControl(ctx); await peerReload(ctx); await noControl(ctx);
    },
  ],{scope:'agent'}),
  define('machine.reconnect-display',[
    async ctx=>{await screen(ctx); await pixels(ctx,'desktop');},
    async ctx=>{await button(panel(ctx),'Reconnect').click(); await pixels(ctx,'desktop'); await noControl(ctx);},
    async ctx=>{
      await button(panel(ctx),'Full screen').click();
      await waitFor(()=>ctx.state.b.page.evaluate(()=>Boolean(document.fullscreenElement)),'fullscreen_enter');
      ctx.assert(await ctx.state.b.page.evaluate(()=>document.fullscreenElement?.id==='machine-desktop'),'fullscreen_exact_panel');
      await button(panel(ctx),'Exit full screen').click();
      await waitFor(()=>ctx.state.b.page.evaluate(()=>document.fullscreenElement===null),'fullscreen_exit');
    },
    async ctx=>{
      await button(ctx.state.b.page,'Close machine desktop').click(); await absent(ctx,panel(ctx),'desktop_closed');
      await peerReload(ctx); await button(ctx.state.b.page,'Show Agent computer').click(); await pixels(ctx,'desktop');
    },
    async ctx=>{await noControl(ctx); const f=ctx.state.f, state=await f.runtime.displaySessions({agentId:f.agentId});
      ctx.assert(state.agent_id===f.agentId && state.active===1 && state.closed>=1,'display_old_session_closed');},
  ],{scope:'agent'}),
  define('browser.enable',[
    async ctx=>{await screen(ctx,'browser'); await visible(ctx,button(ctx.state.b.page,'Enable browser'),'browser_initially_disabled');},
    async ctx=>{await button(ctx.state.b.page,'Enable browser').click(); await visible(ctx,ctx.state.b.page.getByRole('region',{name:'Browser observation'}),'browser_enabled_panel');},
    async ctx=>{await pixels(ctx,'browser'); await visible(ctx,ctx.state.b.page.getByRole('figure',{name:'Live browser page'}),'browser_fresh_frame');},
    async ctx=>{
      const f=ctx.state.f;need(f.runtime?.browserConfigurations && f.runtime?.revokeBrowser,'browser_runtime_fixture_missing');
      const writes=await f.runtime.browserConfigurations({agentId:f.agentId});
      ctx.assert(writes.length===1 && writes[0].incarnation===f.runtimeGeneration && writes[0].enabled===true,'browser_configuration_exact_incarnation');
      const denied=await f.runtime.revokeBrowser({agentId:f.agentId});ctx.defer('restore_browser_capability',()=>denied.restore());
      await peerReload(ctx); await button(ctx.state.b.page,'Show Agent computer').click();
      await ctx.state.b.page.getByRole('tab',{name:'Browser',exact:true}).click();
      await visible(ctx,ctx.state.b.page.getByText(/Browser access is not permitted|Browser is not supported/),'browser_refusal_explicit');
    },
  ],{scope:'agent'}),
  define('browser.observe',[
    async ctx=>{await screen(ctx,'browser');await visible(ctx,ctx.state.b.page.getByRole('region',{name:'Browser observation'}),'browser_observer_open');},
    async ctx=>{
      await pixels(ctx,'browser'); const f=ctx.state.f;
      const location=ctx.state.b.page.locator('[aria-label="Browser location"]');
      await contains(ctx,location,f.browserAddress.replace(/^https?:\/\//,''),'browser_location_exact');
      const pause=await f.runtime.pauseFrames({agentId:f.agentId});ctx.defer('resume_browser_frames',()=>pause.restore());
      await visible(ctx,ctx.state.b.page.getByRole('figure',{name:'Last browser frame'}),'browser_stale_label');
      await contains(ctx,ctx.state.b.page.getByRole('region',{name:'Browser observation'}),'Last frame · Reconnecting…','stale_not_live');await pause.restore();
    },
    async ctx=>{
      await button(ctx.state.b.page,'Close computer').click();
      await absent(ctx,ctx.state.b.page.getByRole('region',{name:'Browser observation'}),'browser_closed');
      const state=await ctx.state.f.runtime.browserObservers({agentId:ctx.state.f.agentId});
      ctx.assert(state.active===0,'browser_observer_released');
    },
    async ctx=>{
      const f=ctx.state.f;await peerReload(ctx);await button(ctx.state.b.page,'Show Agent computer').click();
      await ctx.state.b.page.getByRole('tab',{name:'Browser',exact:true}).click();
      const revoke=await f.runtime.revokeBrowser({agentId:f.agentId});ctx.defer('restore_browser_observation',()=>revoke.restore());
      await visible(ctx,ctx.state.b.page.getByText('Browser access is not permitted.',{exact:true}),'browser_authority_loss');
      await waitFor(async()=> (await f.runtime.browserObservers({agentId:f.agentId})).active===0,'revoked_observer_released',30000,2000);
      ctx.assert((await f.runtime.browserObservers({agentId:f.agentId})).active===0,'browser_does_not_outlive_authority');
    },
  ],{scope:'agent'}),
];
