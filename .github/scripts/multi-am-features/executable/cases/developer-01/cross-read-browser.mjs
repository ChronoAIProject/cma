import {id, required, evidence, apiPath, browser} from './common.mjs';
import {digest} from './access-common.mjs';

function readOnly(ctx, receipt) {
  ctx.assert(receipt.requests.every(r => ['GET', 'HEAD', 'OPTIONS'].includes(r.method)),
    'admin_cross_read_browser_mutation');
}

export async function readOperatorBrowser(ctx, f, am = ctx.am) {
  let session;
  // Register before the shared close hook so final assertions run after drain,
  // including on an earlier case failure. Preserve the full serving receipts.
  ctx.defer('admin_cross_read_browser_finality', async () => {
    if (session) readOnly(ctx, await evidence(ctx, session));
  });
  session = await browser(ctx, 'read_operator', am, f.agentId);
  return session;
}

// Use the actual Agent computer/Files controls. A navigation URL alone does not
// open the file preview, and requested ingress is not download-serving proof.
export async function readOperatorDownloads(ctx, f, session) {
  const parts = f.filePath?.split('/');
  required(parts?.length && parts.every(part => part && part !== '.' && part !== '..'
    && !part.includes('\\')), 'admin_cross_read_relative_file_required');
  const endpoint = '/api/v1/agents/' + id(f.agentId) + '/files/download';
  for (const am of ['a', 'b']) {
    await session.switchAM(am);
    await session.page.goto(new URL('/agents/' + id(f.agentId), session.origin).href);
    await session.page.getByRole('button', {name: 'Show Agent computer', exact: true}).click();
    await session.page.getByRole('tab', {name: 'Files', exact: true}).click();
    const tree = session.page.getByRole('list', {name: 'Home files', exact: true});
    await tree.waitFor();
    for (const part of parts) await tree.getByRole('button', {name: part, exact: true}).click();
    const [response, download] = await Promise.all([
      session.page.waitForResponse(r => r.request().method() === 'GET' && apiPath(r.url()) === endpoint
        && new URL(r.url()).searchParams.get('path') === f.filePath),
      session.page.waitForEvent('download'),
      session.page.getByRole('button', {name: 'Download file', exact: true}).click(),
    ]);
    const stream = await download.createReadStream();
    ctx.assert(stream && download.suggestedFilename() === parts.at(-1), 'admin_cross_read_download_identity');
    const chunks = []; let size = 0;
    for await (const chunk of stream) {
      size += chunk.length; ctx.assert(size <= 1048576, 'admin_download_bound'); chunks.push(chunk);
    }
    ctx.assert(digest(Buffer.concat(chunks)) === digest(f.content), 'admin_cross_owner_download_bytes');
    const requestId = await response.headerValue('x-request-id');
    ctx.assert(response.status() === 200 && requestId?.trim(), 'admin_cross_read_download_response');
    const receipt = await evidence(ctx, session);
    readOnly(ctx, receipt);
    ctx.assert(receipt.requests.some(r => r.method === 'GET' && r.status === 200 && r.am === am
      && r.serving_am === am && r.request_id === requestId), 'admin_cross_read_download_ingress');
    await ctx.observe('admin_cross_read_download', async () => ({request_id: requestId,
      am, file_sha256: digest(f.content), agent_id_sha256: digest(f.agentId)}));
  }
}
