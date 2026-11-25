const assert = require('node:assert/strict');
const { spawn, execFileSync } = require('node:child_process');
const { createHash } = require('node:crypto');
const { mkdtempSync, mkdirSync, readFileSync, writeFileSync, rmSync, symlinkSync } = require('node:fs');
const { createServer } = require('node:http');
const { tmpdir } = require('node:os');
const { join } = require('node:path');
const { setTimeout: delay } = require('node:timers/promises');
const { test } = require('node:test');
const { processRpc, stop } = require('./native_tools_fixture.cjs');

function number(value, width = 4) {
  const bytes = Buffer.alloc(width);
  if (width === 8) bytes.writeBigUInt64BE(BigInt(value));
  else bytes.writeUInt32BE(value);
  return bytes;
}

function field(value) {
  const bytes = Buffer.from(value);
  return Buffer.concat([number(bytes.length), bytes]);
}

function digest(...values) {
  return createHash('sha256').update(Buffer.concat(values)).digest('hex');
}

function prepareRequest(operation, identity, name, reference = 'proof/fixed') {
  const url = 'https://github.com/example/project';
  return { t: 'prepare_workspace', operation_id: operation, workspace_id: identity, name,
    clone_depth: 50, source: { type: 'repository_snapshot', url, reference },
    request_sha256: digest(Buffer.from('chrono.workspace.prepare.v2\0'), number(50), field(identity),
      field(name), Buffer.from([3]), field(url), Buffer.from([1]), field(reference)) };
}

test('candidate runtime pins repository ref and native first task reads that checkout after recovery', {
  timeout: 120000,
  skip: !process.env.CMA_RUNTIME_TEST_BINARY || !process.env.CMA_CODEX_TEST_BINARY || !process.env.CMA_ACP_TEST_BRIDGE,
}, async () => {
  assert.match(execFileSync(process.env.CMA_CODEX_TEST_BINARY, ['--version'], { encoding: 'utf8' }).trim(), /^codex-cli 0\.(147|165)\.0$/);
  assert(['84f9670793e2848b6c49e0541a325223239eeabd0e406d9a586dbcd2935cb85c', '11c84d6c37f3b2e15020405a2301df3ed8c16ed2d9b05c3a3a43620bba78a7d4'].includes(createHash('sha256').update(readFileSync(process.env.CMA_ACP_TEST_BRIDGE)).digest('hex')));
  const directory = mkdtempSync(join(tmpdir(), 'cma-repository-task-'));
  const source = join(directory, 'source');
  const home = join(directory, 'home');
  const agentHome = join(directory, 'agent-home');
  for (const path of [source, home, agentHome]) mkdirSync(path);
  const git = (...args) => execFileSync('git', args, { cwd: source, encoding: 'utf8' }).trim();
  git('init', '-b', 'proof/fixed');
  git('config', 'user.name', 'Repository proof');
  git('config', 'user.email', 'proof@example.test');
  writeFileSync(join(source, 'proof.txt'), 'FIXED_SOURCE_04\n');
  git('add', 'proof.txt');
  git('commit', '-m', 'fixed source');
  const sourceCommit = git('rev-parse', 'HEAD');
  const archive = execFileSync('git', ['archive', '--format=zip', '--prefix=source/', sourceCommit], { cwd: source });
  writeFileSync(join(source, 'proof.txt'), 'MOVED_REF_04\n');
  git('commit', '-am', 'move ref');
  const movedCommit = git('rev-parse', 'HEAD');
  assert.notEqual(movedCommit, sourceCommit);
  symlinkSync('../../existing.txt', join(source, 'escape'));
  git('add', 'escape');
  git('commit', '-m', 'unsafe link fixture');
  const unsafeCommit = git('rev-parse', 'HEAD');
  const unsafeArchive = execFileSync('git', ['archive', '--format=zip', '--prefix=source/', unsafeCommit], { cwd: source });
  writeFileSync(join(home, 'existing.txt'), 'PRESERVE_EXISTING_04');
  let resolutions = 0;
  let archives = 0;
  let modelRound = 0;
  let boundaryError;
  let checkoutCommit;
  let sourceBehavior = 'ready';
  let partialArchiveRequested = false;
  const task = 'Read proof.txt and HEAD, then write first-task.txt with TASK_EXECUTED_04.';
  const server = createServer(async (request, response) => {
    try {
      const url = new URL(request.url, 'http://localhost');
      if (url.pathname.startsWith('/api/v1/proxy/s/api-github/')) {
        assert.equal(request.headers.authorization, 'Bearer repository-proof-local');
        if (sourceBehavior === 'inaccessible') { response.writeHead(404).end(); return; }
        if (url.pathname.endsWith('/repos/example/project')) {
          response.setHeader('content-type', 'application/json');
          response.end(JSON.stringify({ default_branch: 'proof/fixed' }));
          return;
        }
        if (url.pathname.endsWith('/commits')) {
          if (sourceBehavior === 'missing-ref') { response.writeHead(404).end(); return; }
          assert.equal(url.searchParams.get('sha'), 'proof/fixed');
          resolutions += 1;
          response.setHeader('content-type', 'application/json');
          response.end(JSON.stringify([{ sha: sourceBehavior === 'unsafe' ? unsafeCommit : sourceBehavior === 'partial' || resolutions === 1 ? sourceCommit : movedCommit }]));
        } else if (url.pathname.endsWith('/zipball/' + unsafeCommit)) {
          response.setHeader('content-type', 'application/zip');
          response.end(unsafeArchive);
        } else if (url.pathname.endsWith('/zipball/' + sourceCommit)) {
          if (sourceBehavior === 'partial') { partialArchiveRequested = true; return; }
          archives += 1;
          response.setHeader('content-type', 'application/zip');
          response.end(archive);
        } else { response.writeHead(404).end(); }
        return;
      }
      assert.equal(url.pathname, '/v1/responses');
      let text = '';
      for await (const chunk of request) text += chunk;
      const body = JSON.parse(text);
      const declared = body.tools ?? body.input.filter((item) => item.type === 'additional_tools').flatMap((item) => item.tools);
      let output;
      if (modelRound === 0) {
        assert(JSON.stringify(body.input).includes(task));
        const execution = declared.find((tool) => tool.name === 'functions')?.tools?.find((tool) => tool.name === 'exec');
        const command = { cmd: 'cat proof.txt; git rev-parse HEAD; printf TASK_EXECUTED_04 > first-task.txt' };
        if (execution) {
          const metadata = JSON.parse(body.client_metadata['x-codex-turn-metadata']);
          if (metadata.code_mode_tool_names) assert(Object.hasOwn(metadata.code_mode_tool_names, 'exec_command'));
        } else assert(declared.some((tool) => tool.name === 'exec_command'));
        output = [{ id: 'fc_repository_read', call_id: 'call_repository_read', status: 'completed',
          ...(execution ? { type: 'custom_tool_call', namespace: 'functions', name: 'exec', input: "const entry = ALL_TOOLS.find(t => t.name === 'exec_command'); if (!entry) throw new Error('Missing native command tool'); text(await tools[entry.name](" + JSON.stringify(command) + '));' }
            : { type: 'function_call', name: 'exec_command', arguments: JSON.stringify(command) }) }];
      } else {
        const results = body.input.filter((item) => ['function_call_output', 'custom_tool_call_output'].includes(item.type));
        assert(JSON.stringify(results).includes('FIXED_SOURCE_04'), JSON.stringify(results));
        assert(JSON.stringify(results).includes(checkoutCommit), JSON.stringify(results));
        output = [{ type: 'message', id: 'msg_repository_done', role: 'assistant', status: 'completed',
          content: [{ type: 'output_text', text: 'Read fixed checkout and executed first task.', annotations: [] }] }];
      }
      modelRound += 1;
      response.writeHead(200, { 'content-type': 'text/event-stream' });
      const send = (event) => response.write('data: ' + JSON.stringify(event) + '\n\n');
      send({ type: 'response.created', response: { id: 'resp_repo_' + modelRound, status: 'in_progress', output: [] } });
      for (const [index, item] of output.entries()) {
        send({ type: 'response.output_item.added', output_index: index, item });
        send({ type: 'response.output_item.done', output_index: index, item });
      }
      send({ type: 'response.completed', response: { id: 'resp_repo_' + modelRound, status: 'completed', output,
        usage: { input_tokens: 1, output_tokens: 1, total_tokens: 2 } } });
      response.end();
    } catch (error) {
      boundaryError ??= error;
      if (!response.headersSent) response.writeHead(500);
      response.end();
    }
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  const origin = 'http://127.0.0.1:' + server.address().port;
  const controlOrigin = 'http://127.0.0.1:19404';
  let runtime;
  let bridge;
  const control = async (frame) => {
    const response = await fetch(controlOrigin + '/runtime/control', {
      method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(frame),
      signal: AbortSignal.timeout(30000), redirect: 'error',
    });
    const result = await response.json();
    assert.equal(response.status, 200, JSON.stringify(result));
    return result;
  };
  const start = async () => {
    runtime = spawn(process.env.CMA_RUNTIME_TEST_BINARY, [], { detached: true, stdio: 'ignore',
      env: { PATH: process.env.PATH, TMPDIR: directory, CHRONO_RUNTIME_HOME: home,
        CHRONO_RUNTIME_STATE_ROOT: join(directory, 'state'), CHRONO_SANDBOX_ID: 'repository-proof',
        CHRONO_RUNTIME_GENERATION: '1', CHRONO_RUNTIME_BIND: '127.0.0.1:19404',
        CHRONO_PLATFORM_BASE_URL: origin, CHRONO_PLATFORM_PROXY_BIND: '127.0.0.1:19405',
        CHRONO_SANDBOX_REACHES: 'api-github' } });
    for (let attempt = 0; ; attempt++) {
      try { const response = await fetch(controlOrigin + '/ready'); assert(response.ok); break; }
      catch { assert(attempt < 60, 'runtime did not start'); await delay(100); }
    }
    const installDigest = digest(Buffer.from('chrono.install.request.v2\0'), number(30, 8), number(60, 8), number(4096), number(0));
    assert.equal((await control({ t: 'install', operation_id: 'repository-install', request_sha256: installDigest,
      commands: [], timeout_s: 30, total_timeout_s: 60, output_limit: 4096 })).t, 'install');
    assert.equal((await control({ t: 'credential', operation_id: 'repository-credential',
      value: 'repository-proof-local', custody_generation: 1 })).installed, true);
  };
  try {
    await start();
    const request = prepareRequest('workspace:wks_fixed:prepare:v1', 'wks_fixed', 'repository');
    const prepared = await control(request);
    assert.equal(prepared.t, 'prepare_workspace', JSON.stringify(prepared));
    assert.equal(prepared.source_commit, sourceCommit);
    checkoutCommit = prepared.resolved_commit;
    assert.match(checkoutCommit, /^[a-f0-9]{40}$/);
    await stop(runtime);
    await start();
    assert.deepEqual(await control(request), prepared);
    assert.equal(resolutions, 1);
    assert.equal(archives, 1);
    assert.equal(readFileSync(join(home, 'existing.txt'), 'utf8'), 'PRESERVE_EXISTING_04');
    const conflict = await control(prepareRequest('workspace:wks_other:prepare:v1', 'wks_other', 'repository'));
    assert.equal(conflict.t, 'failure');
    assert.equal(readFileSync(join(home, 'repository/proof.txt'), 'utf8'), 'FIXED_SOURCE_04\n');
    const worktree = await control({ t: 'add_worktree', authority_fence: 1, request: {
      operation_id: 'worktree:repository:wt-1:add:v1', workspace: 'repository', name: 'wt-1', base: checkoutCommit,
      request_sha256: digest(Buffer.from('chrono.worktree.add.v1\0'), field('repository'), field('wt-1'), field(checkoutCommit)),
    } });
    assert.equal(worktree.t, 'add_worktree', JSON.stringify(worktree));
    const cwd = worktree.receipt.path;
    assert.equal(worktree.receipt.base_commit, checkoutCommit);
    writeFileSync(join(agentHome, 'config.toml'), ['model = "gpt-5.6-sol"', 'model_provider = "proof"',
      'web_search = "disabled"', 'approval_policy = "never"', 'sandbox_mode = "danger-full-access"',
      '[model_providers.proof]', 'name = "Repository proof"', 'base_url = "' + origin + '/v1"',
      'wire_api = "responses"', 'requires_openai_auth = false'].join('\n'));
    bridge = processRpc(process.execPath, [process.env.CMA_ACP_TEST_BRIDGE], { cwd,
      env: { PATH: process.env.PATH, CODEX_HOME: agentHome, CODEX_PATH: process.env.CMA_CODEX_TEST_BINARY } });
    await bridge.call('initialize', { protocolVersion: 1, clientCapabilities: {} });
    const conversation = await bridge.call('session/new', { cwd, mcpServers: [] });
    await bridge.call('session/set_mode', { sessionId: conversation.sessionId, modeId: 'agent-full-access' });
    await bridge.call('session/prompt', { sessionId: conversation.sessionId, prompt: [{ type: 'text', text: task }] });
    if (boundaryError) throw boundaryError;
    assert.equal(readFileSync(join(cwd, 'first-task.txt'), 'utf8'), 'TASK_EXECUTED_04');
    assert.equal(readFileSync(join(cwd, 'proof.txt'), 'utf8'), 'FIXED_SOURCE_04\n');
    assert.equal(execFileSync('git', ['rev-parse', 'HEAD'], { cwd, encoding: 'utf8' }).trim(), checkoutCommit);
    assert.equal(readFileSync(join(home, 'existing.txt'), 'utf8'), 'PRESERVE_EXISTING_04');
    assert.equal(modelRound, 2);
    console.log(JSON.stringify({ sourceCommit, checkoutCommit, movedCommit, resolutions, archives, firstTaskExecuted: true }));
    for (const [behavior, code] of [['inaccessible', 'repository_unavailable'], ['missing-ref', 'repository_ref_not_found'], ['unsafe', 'repository_source_invalid']]) {
      sourceBehavior = behavior;
      const identity = 'wks_' + behavior.replaceAll('-', '_');
      const rejectedRequest = prepareRequest('workspace:' + identity + ':prepare:v1', identity, identity);
      const failure = await control(rejectedRequest);
      assert.equal(failure.t, 'failure', JSON.stringify(failure));
      assert.equal(failure.code, code, JSON.stringify(failure));
      assert.equal(readFileSync(join(home, 'existing.txt'), 'utf8'), 'PRESERVE_EXISTING_04');
      assert.equal(readFileSync(join(cwd, 'proof.txt'), 'utf8'), 'FIXED_SOURCE_04\n');
      assert.equal((await control(request)).resolved_commit, checkoutCommit);
    }
    sourceBehavior = 'partial';
    const partial = prepareRequest('workspace:wks_partial:prepare:v1', 'wks_partial', 'partial');
    const interrupted = control(partial).catch(() => undefined);
    for (let attempt = 0; !partialArchiveRequested; attempt++) {
      assert(attempt < 100, 'runtime did not request the pinned archive');
      await delay(20);
    }
    const resolutionsBeforeRestart = resolutions;
    await stop(runtime);
    await interrupted;
    await start();
    const recovery = await control(partial);
    assert.equal(recovery.t, 'failure');
    assert.equal(recovery.code, 'workspace_prepare_outcome_unknown');
    assert.equal(resolutions, resolutionsBeforeRestart);
    assert.deepEqual(await control(request), prepared);
    assert.equal(readFileSync(join(home, 'existing.txt'), 'utf8'), 'PRESERVE_EXISTING_04');
    console.log(JSON.stringify({ interruptedPreparation: recovery.code, reResolvedAfterRestart: false }));
  } finally {
    await bridge?.close();
    await stop(runtime);
    server.closeAllConnections();
    await new Promise((resolve) => server.close(resolve));
    rmSync(directory, { recursive: true, force: true });
  }
});
