import {Blocked} from '../../runner.mjs';
import {define, openBrowser, account, service, visible, invoke, same, digest, uiReceipt, peer} from './helpers.mjs';
import {channelReceipt} from './channel-browser.mjs';

const documents = [
  {id: 'integration', title: 'Integration guide', heading: 'Agent Event Gateway integration guide',
    required: 'Implement a downstream agent provider'},
  {id: 'architecture', title: 'Architecture', heading: 'Event Gateway architecture and provider integration',
    required: 'Full event context and restart recovery'},
  {id: 'protocol', title: 'Protocol reference', heading: 'CMA Event Gateway protocol 1.0',
    required: '8. Tools API and MCP'},
];
const text = value => typeof value === 'string' && value.length > 0;
const maxBytes = 4 * 1024 * 1024;

export function validateDocumentationBinding(session) {
  for (const method of ['egEvidence', 'switchEG', 'networkIdle'])
    if (typeof session[method] !== 'function') throw new Blocked('documentation_public_eg_browser_required');
  const binding = session.documentation;
  let base;
  try {base = new URL(binding?.publicBase);} catch {throw new Blocked('documentation_local_public_base_required');}
  if (base.protocol !== 'http:' || !['127.0.0.1', 'localhost', '[::1]'].includes(base.hostname)
    || base.username || base.password || base.search || base.hash || binding.publicBase.endsWith('/')
    || typeof binding.advertiseCanonical !== 'boolean')
    throw new Blocked('documentation_local_public_base_required');
  return binding;
}

export function markdownOutline(markdown) {
  const headings = []; let fence = null, diagrams = 0;
  for (const line of markdown.split(/\r?\n/)) {
    const boundary = /^\s*(`{3,}|~{3,})(.*)$/.exec(line);
    if (boundary) {
      if (fence && boundary[1][0] === fence[0] && boundary[1].length >= fence.length
        && boundary[2].trim() === '') fence = null;
      else if (!fence) {fence = boundary[1]; if (boundary[2].trim() === 'mermaid') diagrams++;}
      continue;
    }
    const heading = !fence && /^#{1,6}\s+(.+)$/.exec(line);
    if (heading) headings.push(heading[1].replace(/[`*_]/g, '').trim());
  }
  if (fence) throw Error('documentation_unclosed_markdown_fence');
  return {headings, diagrams};
}

export function canonicalMarkdown(value, binding) {
  const canonical = 'https://cma.nyxid.ai/event-gateway';
  if (!binding.advertiseCanonical) value = value.replaceAll(binding.publicBase + '/console', canonical + '/legacy-console');
  return value.replaceAll(binding.publicBase, canonical).replaceAll('\r\n', '\n');
}

// Source prose/code must survive the offered download. Link destinations are
// checked separately because the compiled UI and server rewrite them into
// document-local anchors using different renderers.
const substantiveMarkdown = value => value.replace(/^<a id="[^"]+"><\/a>\n\n/gm, '')
  .replace(/\]\(([^)]+)\)/g, '](link)').trim();

export function assertDocumentationDownload(ctx, contents, sources, binding, headingIds) {
  ctx.assert(contents.startsWith('# Agent Event Gateway\n'), 'documentation_download_preamble');
  const ids = [...contents.matchAll(/^<a id="([^"]+)"><\/a>$/gm)].map(match => match[1]);
  ctx.assert(new Set(ids).size === ids.length && ids.length > 3, 'documentation_download_unique_anchors');
  ctx.assert(Array.isArray(headingIds) && headingIds.length > 3 && headingIds.every(id => ids.includes(id)),
    'documentation_download_all_rendered_headings');
  for (const document of documents) {
    const marker = `<a id="${document.id}"></a>\n\n`;
    ctx.assert(contents.split(marker).length === 2, 'documentation_download_section_once');
    const start = contents.indexOf(marker) + marker.length;
    const following = documents[documents.indexOf(document) + 1];
    const end = following ? contents.indexOf(`\n\n---\n\n<a id="${following.id}"></a>`, start)
      : contents.endsWith('\n\n---\n\n') ? contents.length - 7 : contents.length;
    ctx.assert(end > start && substantiveMarkdown(canonicalMarkdown(contents.slice(start, end), binding))
      === substantiveMarkdown(canonicalMarkdown(sources[document.id], binding)), 'documentation_download_complete_source');
  }
  for (const match of contents.matchAll(/\]\(#([^)]*)\)/g))
    ctx.assert(ids.includes(match[1]), 'documentation_download_anchor_target');
}

export function assertDocumentationStructure(ctx, sources, sections, links) {
  ctx.assert(Array.isArray(sections) && sections.length === 3, 'documentation_three_complete_sections');
  const ids = new Set(documents.map(document => document.id));
  let diagrams = 0;
  for (const document of documents) {
    const section = sections.find(section => section.id === document.id);
    const outline = markdownOutline(sources[document.id]);
    ctx.assert(outline.headings[0] === document.heading && outline.headings.includes(document.required),
      'documentation_expected_canonical_source');
    ctx.assert(section?.label === document.title && same(section.headings.map(heading => heading.text), outline.headings),
      'documentation_all_headings_rendered');
    for (const heading of section.headings) {
      ctx.assert(text(heading.id) && heading.id.startsWith(document.id + '-') && !ids.has(heading.id),
        'documentation_unique_scoped_heading'); ids.add(heading.id);
    }
    ctx.assert(section.diagrams === outline.diagrams && section.unrenderedDiagrams === 0,
      'documentation_all_diagrams_rendered'); diagrams += section.diagrams;
  }
  ctx.assert(diagrams > 0 && Array.isArray(links) && links.length > 3, 'documentation_diagrams_and_navigation_present');
  for (const href of links) ctx.assert(typeof href === 'string' && href.startsWith('#') && ids.has(href.slice(1)),
    'documentation_internal_link_resolves');
}

export function assertDocumentationOpenApi(ctx, body, publicBase) {
  ctx.assert(/^3\./.test(body?.openapi ?? '') && body.info?.title === 'CMA Event Gateway'
    && body.info.version === '1.0' && body.servers?.some(server => server.url === publicBase + '/v1'),
  'documentation_real_gateway_discovery');
  for (const [path, method] of [['/channels', 'get'], ['/channels/{channel_id}', 'get'],
    ['/channels/{channel_id}/test', 'post'], ['/channels/{channel_id}/activity', 'get']])
    ctx.assert(text(body.paths?.[path]?.[method]?.operationId), 'documentation_gateway_operations_present');
}

export async function guardDocumentationReads(ctx, session, binding) {
  const allowed = new Set([session.origin, new URL(binding.publicBase).origin]);
  const pending = new Set(), failures = []; let refused = 0;
  const handler = route => {
    const task = (async () => {
      try {
        const request = route.request(), url = new URL(request.url());
        if (allowed.has(url.origin) && ['GET', 'HEAD', 'OPTIONS'].includes(request.method())) await route.fallback();
        else {refused++; await route.abort('blockedbyclient');}
      } catch (error) {failures.push(error);}
    })();
    pending.add(task); return task.finally(() => pending.delete(task));
  };
  await session.page.route('**/*', handler);
  ctx.defer('remove_documentation_read_guard', async () => {
    const errors = [];
    try {await session.page.unroute('**/*', handler);} catch (error) {errors.push(error);}
    const settled = await Promise.allSettled([...pending]);
    errors.push(...settled.filter(result => result.status === 'rejected').map(result => result.reason), ...failures);
    if (refused) errors.push(Error('documentation_nonlocal_or_mutating_request'));
    if (errors.length) throw new AggregateError(errors, 'documentation_guard_failed');
  });
  return {refused: () => refused};
}

// Execute a public read through the real page/proxy. Page fetches are observable
// by the EG binding; APIRequestContext.get would bypass page routing entirely.
export async function readDocumentation(ctx, session, path, contentType, compiledSource) {
  const url = new URL(path, session.origin).href;
  const response = await session.page.evaluate(async ({url, timeoutMs, maxBytes}) => {
    const controller = new AbortController(), timer = setTimeout(() => controller.abort(), timeoutMs);
    let reader;
    try {
      const response = await fetch(url, {method: 'GET', credentials: 'omit', cache: 'no-store',
        redirect: 'error', signal: controller.signal});
      reader = response.body?.getReader();
      if (!reader) throw Error('documentation_response_body_missing');
      const chunks = []; let length = 0;
      for (;;) {
        const {done, value} = await reader.read(); if (done) break;
        length += value.length;
        if (length > maxBytes) throw Error('documentation_response_size_bound'); chunks.push(value);
      }
      const bytes = new Uint8Array(length); let offset = 0;
      for (const chunk of chunks) {bytes.set(chunk, offset); offset += chunk.length;}
      return {status: response.status, url: response.url,
        contentType: response.headers.get('content-type'), requestId: response.headers.get('x-request-id'),
        body: new TextDecoder('utf-8', {fatal: true}).decode(bytes)};
    } finally {clearTimeout(timer); await reader?.cancel().catch(() => {}); controller.abort();}
  }, {url, timeoutMs: Math.max(1, Math.min(ctx.remainingMs?.() ?? 15000, 15000)), maxBytes});
  ctx.assert(response.status === 200 && response.url === url && response.contentType?.startsWith(contentType),
    'documentation_public_response');
  await invoke(session, 'networkIdle');
  const receipt = channelReceipt(ctx, await invoke(session, 'egEvidence'), response.requestId);
  ctx.assert(receipt.serving.source === compiledSource, 'documentation_web_gateway_source_join');
  await ctx.observe('documentation_eg_read', async () => ({schema: 'multi-eg-request-v1', receipt}));
  await ctx.observe('documentation_content', async () => ({path, body_sha256: digest(response.body)}));
  return {...response, receipt};
}

async function downloadDocument(ctx, session, sources, binding, headingIds) {
  const [download] = await Promise.all([session.page.waitForEvent('download'),
    session.page.getByRole('link', {name: /^Download document/}).click()]);
  let deletion;
  const cleanup = () => deletion ??= download.delete();
  ctx.defer('delete_documentation_download', cleanup);
  ctx.assert(download.suggestedFilename() === 'agent-event-gateway.md' && await download.failure() === null,
    'documentation_download_succeeded');
  const stream = await download.createReadStream();
  ctx.assert(stream !== null, 'documentation_download_stream');
  const chunks = []; let size = 0;
  try {
    for await (const chunk of stream) {size += chunk.length; ctx.assert(size <= maxBytes, 'documentation_download_size_bound'); chunks.push(chunk);}
  } finally {stream.destroy();}
  const contents = Buffer.concat(chunks).toString('utf8');
  assertDocumentationDownload(ctx, contents, sources, binding, headingIds);
  await ctx.observe('documentation_download', async () => ({bytes: size, content_sha256: digest(contents)}));
  await cleanup();
}

async function inspectDocumentation(ctx, session, sources, navigationName, container) {
  const page = session.page, nav = page.getByRole('navigation', {name: navigationName, exact: true});
  for (const document of documents) {
    await nav.getByRole('link', {name: document.title, exact: true}).click();
    ctx.assert(new URL(page.url()).hash === '#' + document.id, 'documentation_section_navigation');
    const section = page.locator(`${container} > section#${document.id}`);
    await visible(ctx, section, 'documentation_section_visible');
    for (const image of await section.locator('img').all()) {
      await image.scrollIntoViewIfNeeded(); await image.evaluate(node => node.decode());
      ctx.assert(await image.evaluate(node => node.complete && node.naturalWidth > 0 && node.naturalHeight > 0
        && node.closest('a')?.href === node.src && Number(node.getAttribute('width')) > 0
        && Number(node.getAttribute('height')) > 0), 'documentation_diagram_decoded_and_linked');
    }
  }
  const sections = await page.locator(`${container} > section`).evaluateAll(elements => elements.map(section => ({
    id: section.id, label: section.getAttribute('aria-label'),
    headings: [...section.querySelectorAll('h2,h3,h4,h5,h6')].map(heading => ({id: heading.id, text: heading.textContent.trim()})),
    diagrams: section.querySelectorAll('img').length, unrenderedDiagrams: section.querySelectorAll('code.language-mermaid').length,
  })));
  const links = await page.locator(`${container} a[href^="#"], nav[aria-label="${navigationName}"] a[href^="#"]`)
    .evaluateAll(anchors => anchors.map(anchor => anchor.getAttribute('href')));
  assertDocumentationStructure(ctx, sources, sections, links);
  const internal = nav.locator('a[href^="#protocol-"]').first(), href = await internal.getAttribute('href');
  await internal.click();
  ctx.assert(new URL(page.url()).hash === href && await page.locator(href).count() === 1,
    'documentation_internal_heading_navigation');
  await uiReceipt(ctx, page);
  return sections.flatMap(section => section.headings.map(heading => heading.id));
}

export const documentationCase = {...define(4, 'browser', ['browser', 'eg_browser'], async ctx => {
  let session, binding, source, guard, compiledUrl;
  const sources = {}, replicas = new Set();
  await ctx.step('step_1', async () => {
    session = await openBrowser(ctx, 'user', {documentation: true, initialPath: 'about:blank'});
    binding = validateDocumentationBinding(session); guard = await guardDocumentationReads(ctx, session, binding);
    await session.page.goto(new URL('/event-gateway', session.origin).href);
    await visible(ctx, session.page.getByRole('navigation', {name: 'Event Gateway navigation'}), 'documentation_initial_gateway_context');
    await account(session.page);
    await session.page.getByRole('menu', {name: 'Account', exact: true}).getByRole('menuitem', {name: 'Docs', exact: true}).click();
    ctx.assert(new URL(session.page.url()).pathname === '/docs', 'documentation_account_docs_route');
    await session.page.getByRole('link', {name: /Event Gateway Protocol/}).click();
    await visible(ctx, session.page.getByRole('heading', {name: 'Event Gateway Protocol', exact: true}), 'documentation_compiled_reference');
    ctx.assert(new URL(session.page.url()).pathname === '/docs/event-gateway', 'documentation_reference_route');
    source = (await invoke(ctx.fixture('browser'), 'evidence', session.page)).compiled_source;
    ctx.assert(/^[a-f0-9]{40}$/.test(source ?? ''), 'documentation_compiled_source');
  });
  await ctx.step('step_2', async () => {
    for (let iteration = 0; iteration < 2; iteration++) {
      let actual;
      for (const document of documents) {
        const response = await readDocumentation(ctx, session, '/event-gateway/docs/' + document.id + '.md', 'text/markdown', source);
        if (actual === undefined) actual = response.receipt.replica;
        ctx.assert(response.receipt.replica === actual, 'documentation_read_actual_replica');
        if (iteration === 0) sources[document.id] = response.body;
        else ctx.assert(response.body === sources[document.id], 'documentation_peer_same_markdown');
      }
      const api = await readDocumentation(ctx, session, '/event-gateway/openapi.json', 'application/json', source);
      ctx.assert(api.receipt.replica === actual, 'documentation_discovery_actual_replica');
      assertDocumentationOpenApi(ctx, JSON.parse(api.body), binding.publicBase);
      replicas.add(actual);
      if (iteration === 0) await session.switchEG(peer(actual));
    }
    ctx.assert(replicas.size === 2, 'documentation_both_actual_eg_replicas');
    const headings = await inspectDocumentation(ctx, session, sources, 'On this page', 'article');
    await downloadDocument(ctx, session, sources, binding, headings); compiledUrl = session.page.url();
    for (const replica of replicas) {
      await session.switchEG(replica);
      const response = await session.page.goto(new URL('/event-gateway/docs', session.origin).href);
      ctx.assert(response?.status() === 200 && response.headers()['content-type']?.startsWith('text/html'),
        'documentation_backend_html_response');
      await invoke(session, 'networkIdle');
      const receipt = channelReceipt(ctx, await invoke(session, 'egEvidence'), response.headers()['x-request-id']);
      ctx.assert(receipt.serving.source === source && receipt.replica === replica, 'documentation_backend_same_source_actual_replica');
      await ctx.observe('documentation_backend_page', async () => ({schema: 'multi-eg-request-v1', receipt}));
      await visible(ctx, session.page.getByRole('heading', {name: 'Agent Event Gateway', exact: true}), 'documentation_backend_readable');
      const headings = await inspectDocumentation(ctx, session, sources, 'Sections', 'main');
      await downloadDocument(ctx, session, sources, binding, headings);
    }
  });
  await ctx.step('expected_result', async () => {
    // Fragment navigation adds history entries. Restore the compiled docs URL,
    // then use the product service selector to return to the gateway context.
    await session.page.goto(compiledUrl);
    await service(session.page, 'Event Gateway');
    await visible(ctx, session.page.getByRole('navigation', {name: 'Event Gateway navigation'}), 'documentation_return_gateway_context');
    ctx.assert(new URL(session.page.url()).pathname === '/event-gateway' && guard.refused() === 0,
      'documentation_return_without_mutations_or_external_reads');
    await uiReceipt(ctx, session.page);
  });
}), evidence_scope: 'eg_only'};
