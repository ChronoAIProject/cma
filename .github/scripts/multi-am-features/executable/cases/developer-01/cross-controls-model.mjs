// Stateful wire/UI model for offline regressions only. No live feature verdict.
import assert from 'node:assert/strict';
import {digest} from './access-common.mjs';

export function controlsModel(options = {}) {
  const calls = [], cleanups = [], observations = [], sessions = [], intents = [], retained = [];
  const originalSettings = {model: 'local-model', reasoning_effort: 'medium',
    permission_mode: options.originalPermissionMode ?? 'guarded'};
  const originalPreferences = {title: 'Original task', system_prompt: '', revision: 1,
    prompt_supported: true, prompt_pending: false};
  let settings = {...originalSettings, pending: false}, preferences = {...originalPreferences};
  let root, workspace, agent, historyText, file, released = 0, fixtureCleaned = 0;
  const config = {actors: Object.fromEntries(['user', 'creator', 'control_user', 'control_operator', 'read_operator']
    .map(role => [role, {subject: role === 'creator' ? 'user-subject' : role + '-subject'}]))};
  const source = 'a'.repeat(40), buildBytes = Buffer.from(JSON.stringify({version: 'fixture', revision: source, local: false}));
  const bindings = {identities: Object.fromEntries(['user', 'control_user', 'control_operator']
    .map(actor => [actor, {actor, subject: config.actors[actor].subject}])),
    compiledWeb: {source, manifestSha256: 'sha256:' + 'b'.repeat(64), buildInfoSha256: digest(buildBytes)},
    userResources: {scope: 'LOCAL_DISPOSABLE_TWO_AM', database_disposable: true, single_matrix_writer: true,
      sandbox_profile: {id: 'profile-fixture', revision: 1}, timeout_ms: 2000,
      reserve: async () => ({parent_cleanup_barrier: true,
        retainIntent: async value => intents.push(value), retainResource: async value => retained.push(value),
        release: async () => {released++; return {released: true};}}),
      finality: async ({sandboxId}) => ({sandbox_id: sandboxId, children_remaining: 0,
        pending_operations: options.pendingAfterRetirement ? 1 : 0, cleanup_complete: true})}};
  const f = {scope: 'LOCAL_DISPOSABLE_TWO_AM', createdByRun: true, agentId: 'agent-fixture',
    targetSettings: {...originalSettings, permission_mode: originalSettings.permission_mode === 'guarded'
      ? 'autonomous' : 'guarded'}, title: 'Changed title',
    prompt: 'Changed prompt', filePath: 'controls-fixture.txt', content: 'private fixture bytes',
    attachmentName: 'attachment-fixture.txt',
    keys: Object.fromEntries(['settings', 'restoreSettings', 'title', 'system_prompt', 'stop']
      .map(name => [name, name + '-key'])), cleanup: async () => {fixtureCleaned++;}};
  const receipt = am => ({request_id: 'request-' + calls.length, serving_am: am,
    serving: {pod_uid: 'model-' + am}, serving_after: {pod_uid: 'model-' + am},
    started_at: 'start', ended_at: 'end', attribution: {status: 'proven'}});
  const adapter = {async open({actor, am, agentId}) {
    assert.equal(agentId, f.agentId);
    let menu = false, responseWait;
    const s = {actor, closed: false, origin: 'http://127.0.0.1:4567',
      switchAM: async () => {}, close: async () => {s.closed = true;},
      page: {async goto() {}, async reload() {}, keyboard: {async press() {menu = false;}},
        request: {async get(url) {
          if (new URL(url).pathname === '/auth/session') return {status: () => 200, json: async () => ({
            authenticated: true, subject: options.wrongCookieSubject ? 'foreign-cookie' : config.actors[actor].subject,
            capabilities: ['control_any_sandbox', ...(actor === 'control_user' || options.controlOperatorCanConfigure
              ? ['use_own_sandboxes'] : [])]})};
          assert.equal(new URL(url).pathname, '/build-info.json');
          return {status: () => 200, body: async () => buildBytes};
        }},
        getByRole(role, {name}) {
          if (role === 'menu') return {async waitFor() {assert.ok(menu);}};
          if (role === 'button') return {async click() {assert.equal(name, 'More Agent actions'); menu = true;}};
          assert.equal(role, 'menuitem'); assert.equal(name, 'Rename');
          return {async count() {assert.ok(menu); return options.legacyAbsentRename && actor !== 'creator' ? 0 : 1;},
            async isVisible() {assert.ok(menu); return true;},
            async isDisabled() {return actor === 'creator' ? !!options.ownerRenameDisabled : !options.sharedRenameEnabled;}};
        },
        waitForResponse(predicate) {
          return new Promise((resolve, reject) => {responseWait = {resolve, reject, predicate};});
        },
        locator(selector) {
          assert.equal(selector, 'input[type="file"]');
          return {async setInputFiles(value) {
            if (options.uploadFails) {responseWait.reject(Error('response_wait_failed')); throw Error('upload_failed');}
            const response = {url: () => 'http://127.0.0.1/api/v1/agents/' + f.agentId + '/attachments',
              request: () => ({method: () => 'POST'}), status: () => 201,
              json: async () => ({id: 'attachment-fixture', agent_id: f.agentId, name: value.name,
                path: '.attachments/fixture', size_bytes: value.buffer.length, sha256: digest(value.buffer)})};
            assert.ok(responseWait.predicate(response)); responseWait.resolve(response);
          }};
        }}};
    sessions.push(s); return s;
  }, evidence: async () => ({requests: [receipt('b')], schema: 'multi-am-browser-evidence-v1',
    real_ui: true, secrets_redacted: true, compiled_source: source, compiled_manifest: bindings.compiledWeb.manifestSha256,
    authentication: 'browser_session'})};
  const ctx = {am: 'a', row: {case_id: 'C275:ADM-082'}, phase: 'before_restart', iteration: 1,
    assert: (value, code) => assert.ok(value, code), step: async (_label, run) => run(),
    defer: (label, run) => cleanups.push({label, run}),
    observe: async (label, run) => {const value = await run(); observations.push({label, value}); return value;},
    fixture: name => name === 'browser' ? adapter : {take: async () => f},
    request: async args => {
      calls.push(structuredClone(args));
      const {method, path, body, actor, allowed = [200]} = args;
      const reply = (status, value) => {
        const am = args.am ?? options.implicitAM ?? 'a';
        const response = {status, body: structuredClone(value), timing: {...receipt(am), am}};
        if (!allowed.includes(status)) throw Object.assign(Error('status_' + status), {response});
        return response;
      };
      if (path === '/api/v1/sandboxes' && method === 'POST') {
        root = {id: 'sandbox-fixture', owner: 'user-subject', name: body.name, status: 'active'};
        return reply(201, root);
      }
      if (path === '/api/v1/sandboxes/sandbox-fixture') {
        if (method === 'DELETE') {
          root.status = 'retired'; if (options.unknownRetire) throw Error('retire_unknown');
          return reply(202, root);
        }
        return reply(200, root);
      }
      if (path.endsWith('/workspaces/inline')) {
        workspace = {id: 'workspace-fixture', sandbox_id: root.id, status: 'ready'}; return reply(201, workspace);
      }
      if (path === '/api/v1/workspaces/workspace-fixture') return reply(200, workspace);
      if (path === '/api/v1/agents') {
        historyText = body.initial_message;
        agent = {id: f.agentId, name: body.name, sandbox: root, workspace, status: 'ready'};
        return reply(202, {id: 'operation-fixture'});
      }
      if (path === '/api/v1/agent-lifecycle/operation-fixture')
        return reply(200, {id: 'operation-fixture', agent_id: f.agentId, status: 'succeeded'});
      if (path.startsWith('/api/v1/resource-access/agent/')) {
        const capabilities = ['view_details', 'view_history', 'view_files'];
        if (actor !== 'read_operator') capabilities.push('operate_agent', 'interact');
        if (options.roleEscalation) capabilities.push('manage_sharing');
        return reply(200, {owner: {kind: 'person', is_current_person: false}, capabilities});
      }
      assert.equal(args.agentId, f.agentId);
      if (path.endsWith('/transcript')) return reply(200, {agent_id: f.agentId,
        turns: [{owner_message: historyText, messages: [], outcome: {type: 'completed'}}]});
      if (path.endsWith('/events/head')) return reply(200, {object: 'cma.agent.event_head',
        agent_id: f.agentId, active_turn: options.busy ? {sequence: 1} : null});
      if (path.endsWith('/settings')) {
        if (actor === 'control_operator') return reply(options.controlReadAllowed ? 200 : 403, {code: 'forbidden'});
        if (method === 'PATCH') {
          if (body.permission_mode === originalSettings.permission_mode && options.unknownRestore === 'settings') {
            if (options.restoreCommitted) settings = {...body, pending: false};
            throw Error('restore_settings_unknown');
          }
          settings = {...body, pending: false};
        }
        return reply(200, {...settings, ...(options.pendingSettings ? {pending: true} : {})});
      }
      if (path.endsWith('/preferences')) {
        if (actor === 'control_operator') return reply(options.controlReadAllowed && method === 'GET' ? 200 : 403, {code: 'forbidden'});
        if (method === 'PATCH') {
          assert.equal(body.expected_revision, preferences.revision);
          const field = Object.keys(body).find(k => k !== 'expected_revision');
          if (body[field] === originalPreferences[field] && options.unknownRestore === field) {
            if (options.restoreCommitted) preferences = {...preferences, [field]: body[field], revision: preferences.revision + 1};
            throw Error('restore_' + field + '_unknown');
          }
          preferences = {...preferences, ...body, revision: preferences.revision + 1}; delete preferences.expected_revision;
        }
        return reply(200, {...preferences, ...(options.unsupportedPrompt ? {prompt_supported: false} : {})});
      }
      if (path.includes('/files/content?')) return file
        ? reply(200, {path: f.filePath, content_base64: Buffer.from(file).toString('base64'), sha256: digest(file)})
        : reply(404, {code: 'file_not_found'});
      if (path.endsWith('/files/tree')) return reply(200, {entries: []});
      if (path.endsWith('/files') && method === 'POST') {
        file = Buffer.from(body.content_base64, 'base64').toString(); return reply(200, {path: body.path, sha256: digest(file)});
      }
      if (path.includes('/files?') && method === 'DELETE') {
        if (options.unknownRestore === 'file') {
          if (options.restoreCommitted) file = undefined;
          throw Error('remove_file_unknown');
        }
        file = undefined; return reply(200, {deleted: true});
      }
      if (path.endsWith('/stop')) {agent = {status: 'stopped', sandbox: {status: 'active'}}; return reply(202, agent);}
      if (path === '/api/v1/agents/' + f.agentId) return reply(200, agent ?? {id: f.agentId, status: 'ready'});
      throw Error('unexpected_model_route');
    }};
  return {ctx, f, config, bindings, calls, observations, sessions, intents, retained,
    get fixtureCleaned() {return fixtureCleaned;}, get released() {return released;}, get root() {return root;},
    get settings() {return settings;}, get preferences() {return preferences;},
    async cleanup() {
      const errors = [];
      for (const {run} of cleanups.toReversed()) {try {await run();} catch (error) {errors.push(error);}}
      return errors;
    }};
}
