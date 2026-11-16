import test from 'node:test';
import assert from 'node:assert/strict';
import { setups } from './setups.mjs';
import { prepareBrowser } from './setup-browser.mjs';
import { resources } from './setup-resources.mjs';
import { digest, setup } from './helpers.mjs';
import { Blocked } from '../../runner.mjs';
import { loadRegistry } from '../../registry.mjs';

// Offline adapter-contract checks only. No application or feature verdict here.
function harness() {
  const calls = [],
    observations = [],
    cleanups = [],
    sessions = [],
    intents = [],
    retained = [];
  const state = { release: 0, reservations: 0, finality: 0, deletes: 0 };
  const build = { version: 'git-' + 'a'.repeat(12), revision: 'a'.repeat(40), local: false };
  const bytes = Buffer.from(JSON.stringify(build));
  const primitives = {
    compiledWeb: {
      source: build.revision,
      manifestSha256: 'sha256:' + 'b'.repeat(64),
      buildInfoSha256: digest(bytes),
    },
    identities: {
      user: { actor: 'user', subject: 'subject-user' },
      outsider: { actor: 'outsider', subject: 'subject-outsider' },
    },
    userResources: {
      scope: 'LOCAL_DISPOSABLE_TWO_AM',
      database_disposable: true,
      single_matrix_writer: true,
      sandbox_profile: { id: 'pinned-profile', revision: 4 },
      async reserve() {
        state.reservations += 1;
        return {
          parent_cleanup_barrier: true,
          retainIntent: async (value) => {
            intents.push(value);
          },
          retainResource: async (value) => {
            retained.push(value);
          },
          release: async () => {
            state.release += 1;
            return { released: true };
          },
        };
      },
      async finality({ sandboxId }) {
        state.finality += 1;
        return {
          sandbox_id: sandboxId,
          children_remaining: 0,
          pending_operations: 0,
          cleanup_complete: true,
        };
      },
    },
  };
  const browser = {
    otherAM: (am) => (am === 'a' ? 'b' : 'a'),
    evidence: async () => ({
      schema: 'multi-am-browser-evidence-v1',
      real_ui: true,
      secrets_redacted: true,
      authentication: 'browser_session',
      compiled_source: primitives.compiledWeb.source,
      compiled_manifest: primitives.compiledWeb.manifestSha256,
      requests: [],
    }),
    async open({ actor, agentId }) {
      const row = { actor, agentId, closed: 0, url: 'about:blank' };
      const page = {
        url: () => row.url,
        goto: async (url) => {
          row.url = url;
        },
        evaluate: async () => ['granted', 'granted'],
        request: {
          async get(url) {
            if (new URL(url).pathname === '/auth/session')
              return {
                status: () => 200,
                json: async () => ({
                  authenticated: true,
                  subject: 'subject-' + actor,
                  capabilities: ['create_sandbox'],
                }),
              };
            return { status: () => 200, body: async () => bytes };
          },
        },
      };
      const session = {
        page,
        origin: 'http://127.0.0.1:31017',
        switchAM: async () => {},
        close: async () => {
          row.closed += 1;
        },
      };
      sessions.push({ ...row, row, session });
      return session;
    },
  };
  const sandboxes = new Map(),
    workspaces = new Map(),
    operations = new Map(),
    agents = new Map();
  let seq = 0;
  async function request(args) {
    calls.push(args);
    const { method, path, body, actor = 'user', am } = args;
    let result,
      status = 200;
    if (method === 'POST') {
      assert.ok(
        intents.some((intent) => intent.key === args.key),
        'custody_before_network',
      );
      if (path === '/api/v1/sandboxes') {
        result = {
          id: 'sandbox-' + ++seq,
          owner: 'subject-' + actor,
          name: body.name,
          status: 'active',
        };
        sandboxes.set(result.id, result);
        status = 201;
      } else if (path.endsWith('/workspaces/inline')) {
        result = {
          id: 'workspace-' + ++seq,
          sandbox_id: path.split('/')[4],
          name: body.name,
          status: 'ready',
          agents: [],
        };
        workspaces.set(result.id, result);
        status = 201;
      } else if (path === '/api/v1/agents') {
        const id = 'operation-' + ++seq,
          agentId = 'agent-' + ++seq;
        const workspace = workspaces.get(body.workspace_id);
        operations.set(id, { id, status: 'succeeded', agent_id: agentId });
        agents.set(agentId, {
          id: agentId,
          status: 'ready',
          title: body.name,
          sandbox: { id: workspace.sandbox_id },
          workspace: { id: workspace.id },
          history: body.initial_message,
        });
        workspace.agents.push({ id: agentId, status: 'ready', task_status: 'completed' });
        result = { id, status: 'provisioning' };
        status = 202;
      } else throw Error('unexpected_post');
    } else if (method === 'DELETE') {
      const root = sandboxes.get(path.split('/').at(-1));
      root.status = 'retired';
      state.deletes += 1;
      result = root;
      status = 202;
    } else if (method === 'GET') {
      if (path.endsWith('/transcript')) {
        const agent = agents.get(args.agentId);
        result = {
          agent_id: agent.id,
          turns: [
            { owner_message: agent.history, turn_sequence: 1, outcome: { type: 'completed' } },
          ],
        };
      } else {
        const id = path.split('/').at(-1);
        result = sandboxes.get(id) ?? workspaces.get(id) ?? operations.get(id) ?? agents.get(id);
        if (result?.owner && result.owner !== 'subject-' + actor) {
          status = 404;
          result = {};
        }
      }
    }
    assert.ok(result, 'known_request');
    const response = {
      status,
      body: { ...result },
      timing: { am, request_id: 'request-' + calls.length },
    };
    if (!args.allowed?.includes(status) && !(args.allowed === undefined && status === 200)) {
      const error = Error('request_status_' + status);
      error.response = response;
      throw error;
    }
    return response;
  }
  const ctx = {
    am: 'a',
    row: { case_id: 'offline-user17' },
    assert: (value, code) => assert.ok(value, code),
    defer: (label, operation) => cleanups.push({ label, operation }),
    observe: async (label, operation) => {
      const value = await operation();
      observations.push({ label, value });
      return value;
    },
    fixture: (name) => {
      assert.equal(name, 'browser');
      return browser;
    },
    request,
  };
  const finish = async () => {
    const failures = [];
    for (const entry of [...cleanups].reverse()) {
      try {
        await entry.operation();
      } catch (error) {
        failures.push(error);
      }
    }
    return failures;
  };
  return {
    ctx,
    primitives,
    browser,
    calls,
    observations,
    cleanups,
    sessions,
    intents,
    retained,
    state,
    finish,
    sandboxes,
    agents,
    bytes,
    build,
  };
}

test('four authentic setup names are registered by the author index', async () => {
  const registry = await loadRegistry();
  assert.deepEqual(
    Object.keys(registry.setups.get('developer-17')).sort(),
    Object.keys(setups).sort(),
  );
  assert.deepEqual(Object.keys(setups).sort(), [
    'account-preferences',
    'agent-public-ready',
    'navigation-tasks',
    'task-draft',
  ]);
});

test('all factories block absent independent artifacts without opening a browser or mutating', async () => {
  for (const take of Object.values(setups)) {
    const h = harness();
    await assert.rejects(take(h.ctx, {}), Blocked);
    assert.equal(h.sessions.length, 0);
    assert.equal(h.calls.length, 0);
  }
});

test('preferences derive exact label/copy text from verified served bytes and real session', async () => {
  const h = harness(),
    value = await setups['account-preferences'](h.ctx, h.primitives);
  assert.equal(value.webBuild.label, h.build.version);
  assert.equal(value.webBuild.copyText, `CMA ${h.build.version} (${h.build.revision})`);
  assert.equal(value.webBuild.provenance, 'compiled-web-manifest');
  assert.equal(new URL(value.session.page.url()).pathname, '/agent-manager');
  assert.equal(h.calls.length, 0);
  await value.cleanup();
  await value.cleanup();
  assert.deepEqual(await h.finish(), []);
  assert.equal(h.sessions[0].row.closed, 1);
  assert.ok(!JSON.stringify(h.observations).includes('subject-user'));
});

test('stale served build bytes and independent compiled source mismatch refuse setup', async () => {
  for (const variant of ['bytes', 'source']) {
    const h = harness();
    if (variant === 'bytes') h.primitives.compiledWeb.buildInfoSha256 = 'c'.repeat(64);
    else
      h.browser.evidence = async () => ({
        schema: 'multi-am-browser-evidence-v1',
        real_ui: true,
        secrets_redacted: true,
        compiled_source: 'd'.repeat(40),
        compiled_manifest: h.primitives.compiledWeb.manifestSha256,
      });
    await assert.rejects(
      prepareBrowser(h.ctx, h.primitives),
      /setup_(build_info_digest|compiled_web_binding)/,
    );
    assert.deepEqual(await h.finish(), []);
    assert.equal(h.sessions[0].row.closed, 1);
  }
});

test('explicit local builds preserve truthful copy text, including absent baked revision', async () => {
  for (const revision of ['', 'a'.repeat(40)]) {
    const h = harness(),
      original = h.browser.open;
    const bytes = Buffer.from(JSON.stringify({ version: 'dev', revision, local: true }));
    h.primitives.compiledWeb.buildInfoSha256 = digest(bytes);
    h.browser.open = async (options) => {
      const value = await original(options),
        get = value.page.request.get;
      value.page.request.get = async (url) =>
        new URL(url).pathname === '/build-info.json'
          ? { status: () => 200, body: async () => bytes }
          : get(url);
      return value;
    };
    const value = await prepareBrowser(h.ctx, h.primitives);
    assert.equal(value.webBuild.label, 'dev (local)');
    assert.equal(
      value.webBuild.copyText,
      revision ? `CMA dev (${revision}) (local)` : 'CMA dev (local)',
    );
    assert.deepEqual(await h.finish(), []);
  }
});

test('bearer auth, wrong browser subject and missing clipboard permissions cannot satisfy preferences', async () => {
  for (const variant of ['bearer', 'subject', 'clipboard']) {
    const h = harness();
    const evidence = h.browser.evidence;
    if (variant === 'bearer')
      h.browser.evidence = async () => ({
        ...(await evidence()),
        authentication: 'real_am_bearer',
      });
    if (variant === 'subject') h.primitives.identities.user.subject = 'different-subject';
    if (variant === 'clipboard') {
      const open = h.browser.open;
      h.browser.open = async (options) => {
        const value = await open(options);
        value.page.evaluate = async () => ['denied', 'granted'];
        return value;
      };
    }
    await assert.rejects(
      setups['account-preferences'](h.ctx, h.primitives),
      /browser_session_authentication_required|setup_session_identity|browser_clipboard_permissions_required/,
    );
    assert.deepEqual(await h.finish(), []);
    assert.equal(h.sessions[0].row.closed, 1);
  }
});

test('prepared session is reused by cases, preserving verified actor and cleanup', async () => {
  const h = harness(),
    value = await setups['account-preferences'](h.ctx, h.primitives);
  const fixture = h.ctx.fixture;
  h.ctx.fixture = (name) =>
    name === 'user-fixtures' ? { take: async () => value } : fixture(name);
  const state = {};
  await setup(h.ctx, state, 'account-preferences');
  assert.equal(state.session, value.session);
  assert.equal(h.sessions.length, 1);
  assert.deepEqual(await h.finish(), []);
  assert.equal(h.sessions[0].row.closed, 1);
});

test('draft requires two genuinely different actors and disposable custody before creates', async () => {
  for (const variant of ['same-subject', 'not-disposable']) {
    const h = harness();
    if (variant === 'same-subject') h.primitives.identities.outsider.subject = 'subject-user';
    else h.primitives.userResources.database_disposable = false;
    await assert.rejects(setups['task-draft'](h.ctx, h.primitives), Blocked);
    assert.equal(h.calls.length, 0);
    assert.equal(h.sessions.length, 0);
  }
});

test('draft uses public Sandbox, actual peer reads and isolated sessions; never submits an Agent', async () => {
  const h = harness(),
    value = await setups['task-draft'](h.ctx, h.primitives);
  assert.match(value.otherLocationPath, /^\/sandboxes\/sandbox-/);
  assert.notEqual(value.session, value.otherSession);
  assert.equal(value.otherActor, 'outsider');
  assert.equal(h.calls.filter((call) => call.method === 'POST').length, 1);
  assert.ok(h.calls.filter((call) => call.method === 'GET').every((call) => call.am === 'b'));
  assert.equal(h.intents.length, 1);
  assert.equal(h.retained[0].id, [...h.sandboxes.keys()][0]);
  await value.cleanup();
  assert.deepEqual(await h.finish(), []);
  assert.equal(h.state.deletes, 1);
  assert.equal(h.state.finality, 1);
  assert.equal(h.state.release, 1);
  assert.ok(h.sessions.every((row) => row.row.closed === 1));
});

test('public Agent setup joins lifecycle and transcript, binds actual Agent browser, cleans once', async () => {
  const h = harness(),
    value = await setups['agent-public-ready'](h.ctx, h.primitives);
  assert.equal(h.retained.filter((row) => row.kind === 'agent')[0].id, value.agentId);
  assert.equal(h.sessions.at(-1).agentId, value.agentId);
  assert.equal(value.historyText, h.agents.get(value.agentId).history);
  const reads = h.calls.filter((call) => call.path.startsWith('/api/v1/agents/'));
  assert.ok(
    reads.length >= 2 && reads.every((call) => call.agentId === value.agentId && call.am === 'b'),
  );
  assert.ok(h.calls.filter((call) => call.method === 'POST').every((call) => call.am === 'a'));
  assert.equal(value.run, undefined);
  assert.equal(value.pass, undefined);
  await value.cleanup();
  assert.deepEqual(await h.finish(), []);
  assert.equal(h.state.deletes, 1);
  assert.equal(h.state.release, 1);
});

test('accepted create with attribution failure retains original ID and cleans without replacement', async () => {
  const h = harness(),
    owned = await resources(h.ctx, h.primitives),
    original = h.ctx.request;
  h.ctx.request = async (args) => {
    const response = await original(args);
    if (args.method === 'POST') {
      const error = Error('request_attribution_missing');
      error.response = response;
      throw error;
    }
    return response;
  };
  await assert.rejects(owned.sandbox(), /request_attribution_missing/);
  assert.equal(h.retained.length, 1);
  await owned.cleanup();
  assert.equal(h.calls.filter((call) => call.method === 'POST').length, 1);
  assert.equal(h.state.deletes, 1);
  assert.equal(h.state.release, 1);
});

test('unknown create stays in parent custody, never retries or releases a clean lease', async () => {
  const h = harness(),
    owned = await resources(h.ctx, h.primitives);
  let attempts = 0;
  h.ctx.request = async () => {
    attempts += 1;
    throw Error('lost_create_receipt');
  };
  await assert.rejects(owned.sandbox(), /lost_create_receipt/);
  await assert.rejects(owned.cleanup(), (error) =>
    error.errors.some((item) => item.message === 'user_setup_uncertain_creation_retained'),
  );
  assert.equal(attempts, 1);
  assert.equal(h.intents.length, 1);
  assert.equal(h.state.release, 0);
});

test('wrong owner or renamed resource is never deleted during cleanup', async () => {
  for (const field of ['owner', 'name']) {
    const h = harness(),
      owned = await resources(h.ctx, h.primitives),
      value = await owned.sandbox();
    h.sandboxes.get(value.id)[field] = 'unrelated';
    await assert.rejects(owned.cleanup(), /user_setup_cleanup_unresolved/);
    assert.equal(h.state.deletes, 0);
    assert.equal(h.state.release, 0);
  }
});

test('404, pending admissions or remaining children cannot prove disposal', async () => {
  for (const variant of ['404', 'pending', 'children']) {
    const h = harness(),
      owned = await resources(h.ctx, h.primitives);
    await owned.sandbox();
    if (variant === '404')
      h.ctx.request = async () => {
        throw Error('request_status_404');
      };
    else
      h.primitives.userResources.finality = async ({ sandboxId }) => ({
        sandbox_id: sandboxId,
        children_remaining: variant === 'children' ? 1 : 0,
        pending_operations: variant === 'pending' ? 1 : 0,
        cleanup_complete: true,
      });
    await assert.rejects(owned.cleanup(), /user_setup_cleanup_unresolved/);
    assert.equal(h.state.release, 0);
  }
});

test('public history rejects failed, duplicate, foreign and truncated transcripts', async () => {
  for (const variant of ['failed', 'duplicate', 'foreign', 'truncated']) {
    const h = harness(),
      original = h.ctx.request;
    h.ctx.request = async (args) => {
      const response = await original(args);
      if (args.path.endsWith('/transcript')) {
        if (variant === 'failed') response.body.turns[0].outcome.type = 'failed';
        if (variant === 'duplicate') response.body.turns.push(response.body.turns[0]);
        if (variant === 'foreign') response.body.agent_id = 'foreign';
        if (variant === 'truncated') response.body.next = 'next';
      }
      return response;
    };
    await assert.rejects(
      setups['agent-public-ready'](h.ctx, h.primitives),
      /user_setup_(history_failed|history_duplicate|transcript_identity)/,
    );
    assert.deepEqual(await h.finish(), []);
    assert.equal(h.state.deletes, 1);
  }
});

test('navigation blocks missing real read fault before any resource or browser allocation', async () => {
  const h = harness();
  await assert.rejects(
    setups['navigation-tasks'](h.ctx, h.primitives),
    /navigation_read_fault_required/,
  );
  assert.equal(h.calls.length, 0);
  assert.equal(h.sessions.length, 0);
});

test('navigation creates real private sentinel and obtains fault control only when case asks', async () => {
  const h = harness(),
    faults = [];
  h.primitives.userResources.navigationReadFault = async (args) => {
    faults.push(args);
    return { restore: async () => {} };
  };
  const value = await setups['navigation-tasks'](h.ctx, h.primitives);
  assert.equal(value.agent.statusLabel, 'Completed');
  assert.equal(value.forbiddenNames.length, 1);
  assert.equal(faults.length, 0);
  assert.equal(
    h.calls.filter((call) => call.method === 'POST' && call.actor === 'outsider').length,
    1,
  );
  await value.faults.failNextNavigationRead();
  assert.equal(faults.length, 1);
  assert.equal(faults[0].count, 1);
  assert.equal(faults[0].method, 'GET');
  assert.equal(faults[0].page, value.session.page);
  await value.cleanup();
  assert.deepEqual(await h.finish(), []);
  assert.equal(h.state.deletes, 2);
  assert.equal(h.state.release, 2);
});
