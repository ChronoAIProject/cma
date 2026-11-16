// Offline public command/Launch model; authentic workers and cleanup stay parent-owned.
import assert from 'node:assert/strict';

export function commandModel(action, options = {}) {
  const calls = [], observations = [], cleanups = [], captures = [], operations = [];
  const original = {triggers: {enabled: false, orchestration_paused: false}, retention: {days: 30}};
  let settings = structuredClone(original), revision = 1, commandRevision, launch, captured = false, cancelled = false, ready = false;
  const f = {scope: 'LOCAL_DISPOSABLE_TWO_AM', createdByRun: true, exclusiveSettings: true,
    triggerId: 'trg_fixture', publicId: 'pub_fixture', adminSubject: 'admin-fixture',
    launchIntent: {actor: 'user', method: 'POST', key: 'launch-once', path: '/api/v1/triggers/public/pub_fixture/launches',
      body: {start_if_ready: true, choice: 'new'}}, cleanup: async () => {},
    custody: async (_intent, dispatch) => {
      operations.push({kind: 'custody', dispatch}); let job;
      return {capture: async response => {captures.push(response); captured = true;},
        cleanup: () => job ??= (async () => {
          operations.push({kind: 'resource_cleanup'});
          if (!captured) throw Error('command_launch_unknown_custody');
          currentLaunch('a'); if (!ready) cancelled = !options.noCancel;
          if (options.cleanupFailure) throw Error('command_native_finality_failed');
        })()};
    }};
  const resolution = state => Object.fromEntries(['sandbox', 'workspace', 'agent'].map(kind =>
    [kind, {state, ...(state === 'pending' ? {} : {id: kind + '-one'})}]));
  const currentLaunch = am => {
    if (!launch) return null;
    const value = structuredClone(launch);
    if (cancelled) {value.state = 'cancelled'; value.revision = 4; return value;}
    if (!settings.triggers.orchestration_paused || ready) {
      ready = true; value.state = options.failedLaunch ? 'failed_terminal' : 'ready';
      value.resolution = resolution('created'); value.ready_at = '2026-11-17T00:00:03Z'; value.revision = 3;
      if (options.differentResources && am === 'b') value.resolution.agent.id = 'foreign-agent';
    } else {
      value.blocked_reason = {type: options.wrongBlock ? 'connection_required' : 'orchestration_paused'};
      value.revision = 2;
      if (options.pausedProvisioning) value.resolution.sandbox = {state: 'created', id: 'early'};
    }
    if (options.differentLaunch) value.id = 'foreign-launch';
    return value;
  };
  const auditRecord = (id, rev, status) => ({id, at: '2026-11-17T00:00:00Z', actor: f.adminSubject,
    action: 'configure', target: {kind: 'resource', id: 'triggers'}, outcome: {status: 'succeeded'},
    detail: {revision: String(rev), status}});
  const ctx = {am: 'a', fixture: () => ({take: async () => f}),
    assert: (ok, code) => assert.ok(ok, code), step: async (_label, run) => run(),
    defer: (label, run) => cleanups.push({label, run}),
    observe: async (label, run) => {const value = await run(); observations.push({label, value}); return value;},
    request: async args => {
      calls.push(structuredClone(args)); const url = new URL(args.path, 'http://fixture.invalid'), path = url.pathname;
      const reply = (status, body) => {
        const response = {status, body: structuredClone(body), timing: {am: args.am, request_id: 'request-' + calls.length,
          serving: {pod_uid: 'model-' + args.am}, serving_after: {pod_uid: 'model-' + args.am},
          started_at: 'start', ended_at: 'end', attribution: {status: 'proven'}}};
        if (!(args.allowed ?? [200]).includes(status)) throw Object.assign(Error('status_' + status), {response});
        return response;
      };
      if (path === '/api/v1/admin/settings') {
        assert.equal(args.actor, 'admin');
        if (args.method === 'PATCH') {
          assert.equal(args.body.expected_revision, revision); settings.triggers = structuredClone(args.body.triggers); revision++;
        } else if (args.method === 'PUT') {
          assert.equal(args.body.expected_revision, revision); settings = structuredClone(args.body.settings); revision++;
          operations.push({kind: 'restore'}); if (options.restoreFailure) throw Error('command_restore_unknown');
        }
        const body = {revision, settings: structuredClone(settings)};
        if (options.peerDrift && commandRevision && args.method === 'GET' && args.am !== ctx.am)
          body.settings.retention.days = 99;
        return reply(200, body);
      }
      if (path === '/api/v1/admin/triggers/' + action) {
        assert.equal(args.method, 'POST'); assert.equal(args.actor, 'admin');
        operations.push({kind: 'command'}); settings.triggers.orchestration_paused = action === 'pause';
        revision++; commandRevision = revision;
        if (options.unknownCommand) throw Error('command_ack_unknown');
        const body = {revision: options.staleRevision ? revision - 1 : revision, settings: structuredClone(settings)};
        if (options.wrongReceiptFlag) body.settings.triggers.orchestration_paused = action !== 'pause';
        const r = reply(options.failedCommand ? 503 : 200, body);
        if (options.commandAttributionFailure) throw Object.assign(Error('command_attribution_failed'), {response: r});
        if (options.unattributedCommand) r.timing.request_id = '';
        if (options.wrongCommandAM) r.timing.am = args.am === 'a' ? 'b' : 'a';
        return r;
      }
      if (path === '/api/v1/admin/activity') {
        assert.equal(args.actor, 'admin');
        const count = options.auditPageCount ?? 3;
        const rows = Array.from({length: count}, (_, i) => auditRecord('old-' + i, 0, 'running'));
        if (commandRevision && !options.missingAudit) {
          const row = auditRecord(options.reusedAuditId ? 'old-0' : 'command-audit', commandRevision,
            action === 'pause' ? 'paused' : 'running');
          if (options.wrongAuditActor) row.actor = 'other-admin';
          if (options.wrongAuditStatus) row.detail.status = 'wrong';
          if (options.wrongAuditRevision) row.detail.revision = '0';
          if (options.wrongAuditTarget) row.target.kind = 'subject';
          if (options.failedAudit) row.outcome.status = 'failed';
          if (options.splitAudit && args.am === 'b') row.id = 'other-audit';
          if (options.reusedAuditId) rows.splice(rows.findIndex(r => r.id === 'old-0'), 1);
          rows.unshift(row);
          if (options.duplicateAudit) rows.unshift({...row, id: 'duplicate-audit'});
        }
        const cursor = Number(url.searchParams.get('cursor') ?? 0), limit = Number(url.searchParams.get('limit'));
        return reply(200, {items: rows.slice(cursor, cursor + limit),
          next: options.repeatedCursor ? '0' : cursor + limit < rows.length ? String(cursor + limit) : null});
      }
      assert.equal(args.actor, 'user');
      if (path === '/api/v1/triggers/trg_fixture') return reply(200, {id: f.triggerId, public_id: f.publicId, availability: 'published'});
      if (path === '/api/v1/triggers/public/pub_fixture') return reply(200, {availability: 'published', always_review: false,
        first_message: '', technical: {services: []}});
      if (path === '/api/v1/launches') {
        const item = currentLaunch(args.am), items = item ? [item] : [];
        if (item && options.duplicateLaunch) items.push({...item, id: 'second-launch'});
        return reply(200, {items});
      }
      if (path === '/api/v1/launches/launch-once') return reply(200, currentLaunch(args.am));
      assert.equal(path, f.launchIntent.path); assert.equal(args.method, 'POST');
      operations.push({kind: 'create'});
      launch = {id: 'launch-once', trigger_id: f.triggerId, public_id: f.publicId, revision_digest: 'a'.repeat(64),
        state: 'resolving', revision: 1, resolution: resolution('pending')};
      if (options.unknownLaunch) throw Error('command_launch_unknown');
      const response = reply(202, launch);
      if (options.launchAttributionFailure) throw Object.assign(Error('command_launch_attribution_failed'), {response});
      return response;
    }};
  return {ctx, f, calls, observations, operations, captures, original, settings: () => settings,
    async cleanup() {
      const errors = [];
      for (const {run} of cleanups.toReversed()) {try {await run();} catch (error) {errors.push(error);}}
      return errors;
    }};
}
