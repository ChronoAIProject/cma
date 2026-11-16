import {readFile,stat} from 'node:fs/promises';
import {define,enter,api,button,visible,contains,absent,enc,hash,peerReload,need,waitFor} from './helpers.mjs';
const root = ctx => '/api/v1/agents/' + enc(ctx.state.f.agentId);
async function openFiles(ctx) {
  await button(ctx.state.b.page,'Show Agent details').click();
  await button(ctx.state.b.page,'Browse files').click();
  await visible(ctx,ctx.state.b.page.getByRole('list',{name:'Home files'}),'file_tree_visible');
}
async function select(ctx,path) {
  const segments = path.split('/'); let prefix = '';
  for (const segment of segments) {
    prefix += (prefix ? '/' : '') + segment;
    const node = ctx.state.b.page.locator('.agent-files__row').filter({hasText:'/home/'+prefix});
    const target = node.getByText(segment,{exact:true});
    if (prefix !== path && await node.getAttribute('aria-expanded') === 'true') continue;
    await target.click();
  }
}
async function preview(ctx,file) {
  await select(ctx,file.path);
  const editor = ctx.state.b.page.getByRole('textbox',{name:'Contents of '+file.name,exact:true});
  await visible(ctx,editor,'file_preview_visible');
  ctx.assert(await editor.inputValue() === file.text,'file_preview_exact_bytes');
  const r = await api(ctx,root(ctx)+'/files/content?'+new URLSearchParams({path:file.path,max_bytes:String(1024*1024)}),{agentId:ctx.state.f.agentId});
  ctx.assert(r.body.path === file.path && r.body.sha256 === hash(file.text)
    && Buffer.from(r.body.content_base64,'base64').toString('utf8') === file.text,'file_preview_path_digest');
}
async function download(ctx,action,expectedHash) {
  const pending = ctx.state.b.page.waitForEvent('download').then(d=>({d}),()=>({error:true}));
  await action(); const {d} = await pending; ctx.assert(Boolean(d),'download_received');
  ctx.defer('delete_private_download',()=>d.delete());
  const path = await d.path(); const info = await stat(path);
  ctx.assert(info.size <= 32*1024*1024,'download_bounded');
  const bytes = await readFile(path); ctx.assert(hash(bytes.toString('base64')) === expectedHash,'download_exact_resource_bytes');
  await ctx.observe('download_hash',async()=>({sha256:hash(bytes.toString('base64')),bytes:bytes.length}));
}
export async function transferFile(page,kind,file) {
  const data = {kind,name:file.name,type:file.mimeType,base64:file.buffer.toString('base64')};
  await page.locator('.agent-composer').evaluate((node,d) => {
    const bytes = Uint8Array.from(atob(d.base64),c=>c.charCodeAt(0));
    const transfer = new DataTransfer(); transfer.items.add(new File([bytes],d.name,{type:d.type}));
    node.dispatchEvent(d.kind === 'paste' ? new ClipboardEvent('paste',{bubbles:true,cancelable:true,clipboardData:transfer})
      : new DragEvent('drop',{bubbles:true,cancelable:true,dataTransfer:transfer}));
  },data);
}
export const cases = [
  define('files.preview',[
    async ctx => {await enter(ctx,'agent-files'); await openFiles(ctx); await preview(ctx,ctx.state.f.textFile);},
    async ctx => {
      const f = ctx.state.f; need(f.changedFile,'changed_file_fixture_missing');
      await button(ctx.state.b.page,'Open changed file '+f.changedFile.path).click();
      await visible(ctx,ctx.state.b.page.getByRole('textbox',{name:'Contents of '+f.changedFile.name}),'changed_file_preview');
      ctx.assert(await ctx.state.b.page.getByRole('textbox',{name:'Contents of '+f.changedFile.name}).inputValue()
        === f.changedFile.text,'changed_file_content');
    },
    async ctx => {
      const f = ctx.state.f; need(f.oversizeFile?.size > 1024*1024,'oversize_preview_fixture_missing');
      await select(ctx,f.oversizeFile.path);
      await contains(ctx,ctx.state.b.page.getByRole('region',{name:'File preview'}),'Download this file to view its contents.','preview_limit_explicit');
      await absent(ctx,ctx.state.b.page.getByRole('textbox',{name:'Contents of '+f.oversizeFile.name}),'oversize_not_in_preview');
    },
    async ctx => {await peerReload(ctx); await openFiles(ctx); await preview(ctx,ctx.state.f.textFile);},
  ],{scope:'agent'}),
  define('files.upload',[
    async ctx => {
      await enter(ctx,'agent-files'); await openFiles(ctx); need(ctx.state.f.directory,'owned_directory_missing');
      await select(ctx,ctx.state.f.directory);
      ctx.assert(await ctx.state.b.page.locator(`.agent-files__row[title="/home/${ctx.state.f.directory}"]`).getAttribute('aria-expanded') === 'true',
        'upload_selected_directory');
    },
    async ctx => {
      const f = ctx.state.f; need(f.uploadFiles?.length,'upload_fixture_missing');
      await ctx.state.b.page.getByLabel('Upload files',{exact:true}).setInputFiles(f.uploadFiles.map(x=>({name:x.name,mimeType:'text/plain',buffer:Buffer.from(x.text)})));
      await visible(ctx,ctx.state.b.page.getByText(`${f.uploadFiles.length} file${f.uploadFiles.length===1?'':'s'} uploaded.`,{exact:true}),'uploads_completed');
    },
    async ctx => {
      for (const f of ctx.state.f.uploadFiles) await preview(ctx,{...f,path:ctx.state.f.directory+'/'+f.name});
      await peerReload(ctx); await openFiles(ctx);
      for (const f of ctx.state.f.uploadFiles) await preview(ctx,{...f,path:ctx.state.f.directory+'/'+f.name});
    },
    async ctx => {
      const {page} = ctx.state.b; const f = ctx.state.f;
      await select(ctx,f.directory);
      await page.getByLabel('Upload files',{exact:true}).setInputFiles({name:'over-limit.txt',mimeType:'text/plain',buffer:Buffer.alloc(32*1024*1024+1)});
      await visible(ctx,page.getByRole('alert').filter({hasText:'32 MiB or smaller'}),'upload_limit_rejected');
      const item = f.uploadFiles[0];
      await page.getByLabel('Upload files',{exact:true}).setInputFiles({name:item.name,mimeType:'text/plain',buffer:Buffer.from('conflicting replacement')});
      await visible(ctx,page.getByRole('alert'),'conflicting_existing_path_not_silent');
      // Current upload uses create-only writes; a conflict is refused. Confirm
      // belongs to explicit editor overwrite/delete, never implicit upload.
      await preview(ctx,{...item,path:f.directory+'/'+item.name});
    },
  ],{scope:'agent'}),
  define('files.message-attachments',[
    async ctx => {
      const {page,f} = await enter(ctx,'agent-files'); need(f.attachmentFiles?.length === 3,'attachment_inputs_fixture_missing');
      const [chosen,pasted,dropped] = f.attachmentFiles.map(v=>({...v,buffer:Buffer.from(v.base64,'base64')}));
      await page.getByLabel('Choose attachments',{exact:true}).setInputFiles(chosen);
      await transferFile(page,'paste',pasted); await transferFile(page,'drop',dropped);
      for (const file of f.attachmentFiles) await visible(ctx,page.getByRole('list',{name:'Attachments'}).getByText(file.name,{exact:true}),'attachment_input_accepted');
    },
    async ctx => {
      const {page} = ctx.state.b, f = ctx.state.f;
      const list = page.getByRole('list',{name:'Attachments'});
      await contains(ctx,list,'Uploading…','attachment_progress_visible');
      await button(page,'Remove '+f.attachmentFiles[2].name).click();
      await absent(ctx,list.getByText(f.attachmentFiles[2].name,{exact:true}),'draft_attachment_removed');
      await f.uploadBarrier.release();
      await waitFor(async()=> (await list.getByRole('status').allTextContents()).every(v=>v==='Ready'),'attachments_ready');
      ctx.assert(await list.getByRole('listitem').count()===2,'only_retained_attachments_ready');
    },
    async ctx => {
      const {page} = ctx.state.b,f=ctx.state.f;
      await page.getByRole('textbox',{name:'Message Agent',exact:true}).fill(f.attachmentMarker);
      await button(page,'Send message').click();
      await visible(ctx,page.getByRole('article').filter({hasText:f.replyMarker}),'attachment_turn_reply');
      const messages = await f.provider.messages({agentId:f.agentId});
      const matching = messages.filter(m=>m.content.includes(f.attachmentMarker));
      ctx.assert(matching.length===1 && matching[0].attachments.length===2
        && new Set(matching[0].attachments.map(a=>a.path)).size===2,'attachments_used_once');
      ctx.assert(matching[0].attachments.every(a=>a.agent_id===f.agentId),'attachments_exact_agent');
    },
    async ctx => {
      const {page} = ctx.state.b,f=ctx.state.f;
      await peerReload(ctx);
      await page.getByLabel('Choose attachments',{exact:true}).setInputFiles({name:'oversize.bin',mimeType:'application/octet-stream',buffer:Buffer.alloc(20*1024*1024+1)});
      await visible(ctx,page.getByRole('alert').filter({hasText:/20 MiB|too large/i}),'attachment_limit_rejected');
      need(f.faults?.failNextUpload,'attachment_failure_fixture_missing');
      const fault = await f.faults.failNextUpload({agentId:f.agentId}); ctx.defer('restore_attachment_upload',()=>fault.restore());
      const retry = f.attachmentFiles[0];
      await page.getByLabel('Choose attachments',{exact:true}).setInputFiles({name:retry.name,mimeType:retry.mimeType,buffer:Buffer.from(retry.base64,'base64')});
      await visible(ctx,button(page,'Retry'),'attachment_retry_visible'); await fault.restore(); await button(page,'Retry').click();
      await contains(ctx,page.getByRole('list',{name:'Attachments'}),'Ready','attachment_retry_completed');
      ctx.assert(await page.getByRole('list',{name:'Attachments'}).getByRole('listitem').count()===1,'attachment_retry_no_duplicate');
    },
  ],{scope:'agent'}),
  define('files.answer-deliverables',[
    async ctx => {
      const {page,f} = await enter(ctx,'agent-deliverables');
      await page.getByRole('textbox',{name:'Message Agent',exact:true}).fill(f.prompt);
      await button(page,'Send message').click();
      await visible(ctx,page.getByRole('article').filter({hasText:f.answerText}).getByRole('list',{name:'Answer deliverables'}),'actual_answer_delivery');
    },
    async ctx => {
      const f=ctx.state.f; await button(ctx.state.b.page,'Preview '+f.delivery.name).click();
      await contains(ctx,ctx.state.b.page.getByRole('complementary',{name:'Deliverable preview'}),f.delivery.text,'delivery_preview_bytes');
      await download(ctx,()=>button(ctx.state.b.page.getByRole('complementary',{name:'Deliverable preview'}),'Download '+f.delivery.name).click(),f.delivery.base64Sha256);
    },
    async ctx => {
      const f=ctx.state.f; need(f.runtime?.removeDelivery,'delivery_remove_control_missing');
      await f.runtime.removeDelivery({agentId:f.agentId,path:f.delivery.path});
      await button(ctx.state.b.page,'Close preview').click(); await peerReload(ctx);
      await visible(ctx,ctx.state.b.page.getByRole('list',{name:'Answer deliverables'}).getByText('File deleted',{exact:true}),'missing_delivery_visible');
    },
    async ctx => {
      const f=ctx.state.f; await button(ctx.state.b.page,'Preview '+f.document.name).click();
      await contains(ctx,ctx.state.b.page.getByRole('complementary',{name:'Deliverable preview'}),`Document revision ${f.document.revision}`,'delivery_document_version');
      await download(ctx,()=>button(ctx.state.b.page.getByRole('complementary',{name:'Deliverable preview'}),'Download '+f.document.name).click(),f.document.base64Sha256);
    },
    async ctx => {
      const f=ctx.state.f;
      for (const v of f.unavailableDeliveries ?? []) await contains(ctx,ctx.state.b.page.getByRole('list',{name:'Answer deliverables'}),v.statusText,'delivery_unavailable_explicit');
      need(f.unavailableDeliveries?.some(v=>v.kind==='denied') && f.unavailableDeliveries.some(v=>v.kind==='too_large'),
        'delivery_denied_oversized_fixtures_missing');
      ctx.assert(f.document.revision>0 && f.delivery.answerId===f.document.answerId,'deliveries_same_answer_exact_versions');
    },
  ],{scope:'agent'}),
];
