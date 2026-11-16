// Offline public-wire model; no runtime, provider or authorization acceptance.
import assert from 'node:assert/strict';
import {digest} from './access-common.mjs';

export function defaultsModel(options = {}) {
  const calls = [], cleanups = [], observations = [], profiles = new Map(), intents = [], retained = [];
  const original = {defaults: Object.fromEntries(['agent', 'workspace', 'sandbox'].map(kind =>
    [kind + '_profile', {id: kind + '-original', revision: 1}])), retention: {agent_log_days: 30},
  inactivity: {auto_pause_enabled: false}, triggers: {enabled: false},
  model_budget: {enabled: false, owners: [], request_models: []}, marketplace: {enabled: options.marketplaceEnabled ?? true}};
  let settings = structuredClone(original), revision = 1, root, fixture, released = 0;
  const config = {actors: {user: {subject: 'owner-fixture'}, admin: {subject: 'admin-fixture'}}};
  const bindings = {defaultsCapacity: {cpu_millis: 1000, memory_mib: 2048},
    userResources: {scope: 'LOCAL_DISPOSABLE_TWO_AM', database_disposable: true, single_matrix_writer: true,
      sandbox_profile: {id: 'parent-profile', revision: 1}, timeout_ms: 2000,
      reserve: async () => ({parent_cleanup_barrier: true,
        retainIntent: async value => intents.push(value),
        retainResource: async value => {retained.push(value); if (options.retainFails) throw Error('retain_failed');},
        release: async () => {released++; return {released: true};}}),
      finality: async ({sandboxId}) => ({sandbox_id: sandboxId, pending_operations: options.nativePending ? 1 : 0,
        children_remaining: 0, cleanup_complete: true})}};
  const snapshot = () => ({revision, settings: structuredClone(settings), model_budget_writes_available: true});
  const ctx = {am: 'a', row: {case_id: 'C275:ADM-050'}, phase: 'before_restart', iteration: 1,
    assert: (value, code) => assert.ok(value, code), step: async (_label, run) => run(),
    defer: (label, run) => cleanups.push({label, run}),
    observe: async (label, run) => {const value = await run(); observations.push({label, value}); return value;},
    fixture: () => ({take: async () => fixture}),
    request: async args => {
      calls.push(structuredClone(args));
      const {method, path, body, actor, allowed = [200]} = args;
      const reply = (status, value) => {
        const response = {status, body: structuredClone(value), timing: {am: args.am ?? 'b',
          request_id: 'request-' + calls.length, serving: {pod_uid: 'model-pod'},
          serving_after: {pod_uid: 'model-pod'}, started_at: 'start', ended_at: 'end', attribution: {status: 'proven'}}};
        if (!allowed.includes(status)) throw Object.assign(Error('status_' + status), {response});
        return response;
      };
      if (path === '/api/v1/admin/settings') {
        assert.equal(actor, 'admin');
        if (method === 'PATCH' || method === 'PUT') {
          assert.equal(body.expected_revision, revision);
          if (method === 'PATCH' && body.defaults) {
            if (Object.values(body.defaults).some(ref => ref.revision === 0)) return reply(422, {code: 'invalid_request'});
            if (Object.values(body.defaults).some(ref => ref.revision > 1) && !options.acceptUnavailable)
              return reply(422, {code: 'invalid_request'});
          }
          if (method === 'PUT') settings = structuredClone(body.settings);
          else {const {expected_revision, ...patch} = body; settings = {...settings, ...structuredClone(patch)};}
          revision++;
        }
        return reply(200, snapshot());
      }
      if (path === '/api/v1/sandboxes?limit=100') return reply(200, {items: options.nonemptyOwner ? [{id: 'prior-root'}] : []});
      if (path === '/api/v1/profile-authoring/bounds') return reply(200, {
        min_cpu_millis: 250, max_cpu_millis: 4000, min_memory_mib: 256, max_memory_mib: 8192});
      if (path.startsWith('/api/v1/marketplace/agent-profiles?')) return settings.marketplace.enabled
        ? reply(200, {items: [...profiles.values()].filter(p => p.kind === 'agent' && p.visibility === 'public')})
        : reply(503, {code: 'marketplace_disabled'});
      const match = path.match(/^\/api\/v1\/(agent|workspace|sandbox)-profiles(?:\/([^/?]+))?(.*)$/);
      if (match) {
        const [, kind, profileId, suffix] = match;
        assert.equal(actor, 'user');
        if (!profileId) {
          assert.equal(method, 'POST');
          const doc = {id: kind + '-fixture-' + profiles.size, kind, owner: 'owner-fixture', name: body.name,
            visibility: 'private', draft_version: 1, draft: body.content};
          profiles.set(doc.id, doc);
          if (options.unknownProfileCreate) throw Error('profile_create_unknown');
          return reply(201, doc);
        }
        const doc = profiles.get(profileId); assert.ok(doc);
        if (suffix === '/revisions') return reply(201, {profile_id: profileId, revision: 1,
          content: options.badRevisionContent ? {} : doc.draft, digest: digest(JSON.stringify(doc.draft))});
        if (suffix.startsWith('/revisions/')) return reply(200, {profile_id: profileId, revision: 1,
          content: doc.draft, digest: digest(JSON.stringify(doc.draft))});
        if (suffix === '/visibility') {doc.visibility = body.visibility; return reply(200, doc);}
        if (suffix === '/retire') {
          doc.retired_at = 'retired';
          if (options.unknownProfileRetire) throw Error('profile_retire_unknown');
          return reply(200, doc);
        }
        return reply(200, doc);
      }
      if (path === '/api/v1/conversations' && method === 'POST') {
        assert.deepEqual(body, {});
        root = {id: 'root-fixture', owner: options.foreignRoot ? 'foreign-owner' : 'owner-fixture',
          profile: structuredClone(settings.defaults.sandbox_profile), status: 'active'};
        const result = {id: 'agent-fixture', object: 'conversation', metadata: {
          sandbox: root.id, workspace: 'workspace-fixture', agent_profile: settings.defaults.agent_profile.id}};
        if (options.unknownCreate) throw Error('conversation_create_unknown');
        const response = reply(200, result);
        if (options.attributionFailure) throw Object.assign(Error('attribution_failed'), {response});
        return response;
      }
      if (path === '/api/v1/agents/agent-fixture') {
        if (!options.allowMissingAttribution) assert.equal(args.agentId, 'agent-fixture');
        return reply(200, {id: 'agent-fixture', profile: settings.defaults.agent_profile,
          workspace: {id: 'workspace-fixture', profile: settings.defaults.workspace_profile}, sandbox: root});
      }
      if (path === '/api/v1/sandboxes/root-fixture') {
        if (method === 'DELETE') {
          root.status = 'retired';
          if (options.unknownRootRetire) throw Error('root_retire_unknown');
          return reply(202, root);
        }
        return reply(200, root);
      }
      throw Error('unexpected_model_route_' + path);
    }};
  return {ctx, config, bindings, calls, cleanups, observations, profiles, intents, retained, original,
    bind(value) {fixture = {...value, scope: 'LOCAL_DISPOSABLE_TWO_AM', createdByRun: true, exclusiveSettings: true}; return fixture;},
    get settings() {return settings;}, get root() {return root;}, get released() {return released;},
    async cleanup() {
      const errors = [];
      for (const {run} of cleanups.toReversed()) {try {await run();} catch (error) {errors.push(error);}}
      return errors;
    }};
}
