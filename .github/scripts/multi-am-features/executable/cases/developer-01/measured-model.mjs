// Offline HTTP/browser contract model. No application, browser or accounting fixture starts here.
import assert from 'node:assert/strict';
import {EventEmitter} from 'node:events';

export function measuredModel(options = {}) {
  const calls = [], observations = [], cleanups = [], actions = [], sessions = [], reports = new Map();
  const original = {usage_reporting: {measured_visible: options.initiallyEnabled ?? false}, retention: {agent_log_days: 30}};
  let settings = structuredClone(original), revision = 1, saves = 0, inStep = false;
  const f = {scope: 'LOCAL_DISPOSABLE_TWO_AM', createdByRun: true, exclusiveSettings: true,
    range: {from: 1794700800, to: 1794704400}, scopes: {
      user: {subject: 'subject-user', sandboxIds: ['sandbox-user']},
      admin: {subject: 'subject-admin', sandboxIds: ['sandbox-user', 'sandbox-admin']}},
    cleanup: async () => {actions.push({op: 'fixture_cleanup'});}};
  const controller = new AbortController();
  const ctx = {am: 'a', signal: controller.signal, remainingMs: () => options.shortBudget ? 500 : 60000,
    assert: (ok, code) => {assert.ok(inStep, 'assertion_outside_step'); assert.ok(ok, code);},
    step: async (_label, run) => {inStep = true; try {return await run();} finally {inStep = false;}},
    defer: (label, run) => cleanups.push({label, run}),
    observe: async (label, run) => {const value = await run(); observations.push({label, value}); return value;},
    fixture: name => name === 'browser' ? adapter : {take: async () => f},
    request: async args => {
      calls.push(structuredClone(args));
      if (args.path === '/api/v1/admin/settings') {
        if (args.method === 'PUT') {
          actions.push({op: 'restore', allClosed: sessions.every(s => s.closed)});
          assert.equal(args.body.expected_revision, revision);
          settings = structuredClone(args.body.settings); revision++;
          if (options.restoreFailure) throw Error('restore_unknown');
        }
        const body = {revision, settings: structuredClone(settings)};
        if (options.stalePeer && saves && args.am !== ctx.am) body.revision--;
        return {status: 200, body, timing: {am: args.am, request_id: 'api-' + calls.length}};
      }
      const report = reports.get(args.path.split('/').at(-1)); assert.ok(report, args.path);
      actions.push({op: 'report_peer', actor: args.actor, am: args.am});
      return {status: 200, body: options.peerChanged ? {...report, as_of: -1} : structuredClone(report),
        timing: {am: args.am, request_id: 'peer-' + calls.length}};
    }};
  const adapter = {
    open: async ({actor, am}) => {
      const index = sessions.length, isSettings = index === 0;
      const s = {actor, am, closed: false, isSettings, receipts: [], records: []}; sessions.push(s);
      let draft, listener, tablePage = 0, report, inventory, point, measured;
      const page = new EventEmitter();
      const emit = (path, body, {method = 'GET', status = 200, requestBody} = {}) => {
        const requestId = 'session-' + index + '-request-' + s.receipts.length;
        const response = {url: () => 'http://127.0.0.1:19900' + path,
          status: () => status, headerValue: async () => options.unjoinedReport && !isSettings ? 'unjoined' : requestId,
          request: () => ({method: () => method, postDataJSON: () => structuredClone(requestBody)}),
          json: async () => {if (options.jsonFailure && !isSettings) throw Error('body_failed'); return structuredClone(body);}};
        s.receipts.push({method, status, request_id: requestId, am,
          serving_am: options.wrongReportServing && !isSettings ? (am === 'a' ? 'b' : 'a') : am,
          serving: {pod_uid: 'model-' + am}, serving_after: {pod_uid: 'model-' + am},
          started_at: 'start', ended_at: 'end', attribution: {status: 'proven'}});
        s.records.push({path, body: structuredClone(body)}); page.emit('response', response); return response;
      };
      const generate = () => {
        measured = settings.usage_reporting.measured_visible;
        const scope = f.scopes[actor], reportId = 'report-' + index;
        const base = '/api/v1/' + (actor === 'admin' ? 'admin/' : '') + 'usage/metrics/reports';
        const summary = {sandbox_count: scope.sandboxIds.length,
          allocated_period: {cpu_core_hours: 24, memory_gib_hours: 48, disk_gib_hours: 72, complete: true},
          total_allocated_capacity: {cpu_cores: 2, memory_bytes: 4 * 2 ** 30, disk_bytes: 8 * 2 ** 30}};
        if (options.changedAllocation && index > 2) summary.allocated_period.cpu_core_hours = 25;
        report = {report_id: reportId, status: options.pendingReport ? 'pending' : 'ready', report_complete: !options.incompleteReport,
          scope: {admin: actor === 'admin', principal: options.wrongPrincipal ? 'foreign' : scope.subject},
          query: {from: f.range.from, to: options.wrongRange ? f.range.to + 1 : f.range.to,
            filters: {owner: null, sandbox_id: null, agent_id: null, runtime_version: null, statuses: [], search: ''}},
          as_of: f.range.to, summary};
        reports.set(reportId, structuredClone(report));
        inventory = {report_id: options.wrongInventoryReport ? 'other' : reportId, next: options.extraPage ? 'next' : null,
          total_count: scope.sandboxIds.length, items: scope.sandboxIds.map(sandbox_id => ({sandbox_id, name: sandbox_id + ' name'}))};
        if (options.foreignSandbox) inventory.items[0].sandbox_id = 'foreign';
        if (options.duplicateSandbox && actor === 'admin') inventory.items[1].sandbox_id = inventory.items[0].sandbox_id;
        emit('/auth/session', {authenticated: true, subject: scope.subject, email: 'not-retained',
          usage_reporting: {measured_visible: options.wrongSessionPolicy ? !measured : measured}});
        if (!options.missingDescriptor) emit(base + (options.directReport ? '' : '/view?window=custom'),
          options.directReport ? report : {report}, options.directReport ? {method: 'POST', status: 200,
            requestBody: report.query} : {});
        emit(base + '/' + reportId + '/sandboxes?limit=50', inventory);
        if (measured || options.hiddenSeries) {
          point = {from: f.range.from, to: f.range.from + 60, mean: options.noGap ? 0 : null, reason: 'no_measurement'};
          const points = options.secondGapPage ? Array.from({length: 50}, (_, i) => ({...point, from: f.range.from + 60 * (i + 1),
            to: f.range.from + 60 * (i + 2), mean: 1})).concat([point]) : [point];
          if (!options.missingSeries) emit(base + '/' + reportId + '/series?metric=cpu&group_by=total', {
            report_id: options.wrongSeriesReport ? 'foreign' : reportId, metric: 'cpu', unit: 'cores', source: 'local accounting',
            lines: [{id: 'total', label: 'Total', points}]});
          if (actor === 'admin' && !options.missingAdminCharts) for (const metric of ['memory', 'disk'])
            emit(base + '/' + reportId + '/series?metric=' + metric + '&group_by=total', {
              report_id: reportId, metric, unit: 'bytes', source: 'local accounting', lines: [{id: 'total', label: 'Total', points}]});
        }
      };
      const loc = (kind, name, at = 0) => ({
        waitFor: async () => {},
        isVisible: async () => ['CPU', 'Memory working set', 'Home PVC disk'].includes(name) ? (options.wrongSummary ? !measured : measured) : name === 'Measured resource charts' ? (options.wrongVisibility ? !measured : measured)
          : options.hiddenAllocation && name === 'Allocated resource-hours' ? false : true,
        count: async () => options.wrongInventoryVisibility ? 1 : (measured ? inventory.items.length * 3 : 0),
        getAttribute: async key => key === 'data-usage-state' ? (options.preview ? 'preview' : 'committed')
          : key === 'data-usage-requested-range' ? 'custom' : 'false',
        getByRole: (role, spec) => loc(role, spec?.name),
        getByText: text => loc('text', text), locator: selector => loc('selector', selector),
        nth: n => loc(kind, name, n),
        allTextContents: async () => {
          if (name === '.usage-resource-card > dd') return [options.wrongNumbers ? '0' : String(report.summary.allocated_period.cpu_core_hours), '48', '72'];
          if (name === '.usage-resource-now dd') return ['2 cores', '4 GiB', '8 GiB'];
          assert.equal(name, '.usage-inventory-name button');
          return inventory.items.map(r => options.wrongRenderedInventory ? 'foreign' : r.name);
        },
        innerText: async () => {
          if (kind !== 'cell') throw Error('unexpected_inner_text');
          if (at === 0) return options.wrongGapRow ? 'foreign interval' : [point.from, point.to]
            .map(value => new Date(value * 1000).toLocaleString('en-SG', {timeZone: 'Asia/Singapore'}) + ' SGT').join(' – ');
          if (at === 1) return options.gapZero ? '0 cores' : 'Unavailable';
          assert.equal(at, 5); return options.wrongGapReason ? 'Observed' : 'no measurement; 0 excluded';
        },
        setChecked: async value => {assert.equal(name, 'Show measured usage'); draft.usage_reporting.measured_visible = value;},
        click: async () => {
          if (name === 'Save settings') {
            saves++;
            const body = {expected_revision: revision, settings: structuredClone(draft)};
            settings = structuredClone(draft); revision++;
            const response = emit('/api/agent-manager/v1/admin/settings', {revision, settings}, {
              method: 'PUT', status: options.failedSave ? 503 : 200,
              requestBody: options.wrongSaveBody ? {...body, expected_revision: 0} : body});
            actions.push({op: 'save', enabled: settings.usage_reporting.measured_visible});
            if (options.wrongSaveServing) s.receipts.at(-1).serving_am = am === 'a' ? 'b' : 'a';
            if (listener) {assert.ok(listener.predicate(response)); listener.resolve(response); listener = null;}
            if (options.unknownSave) throw Error('save_unknown');
          } else if (name === 'Next chart data') tablePage++;
        },
      });
      page.goto = async url => {
        actions.push({op: 'goto', actor, url});
        if (isSettings) {draft = structuredClone(settings); emit('/api/v1/admin/settings', {revision, settings});}
        else generate();
      };
      page.locator = selector => loc('selector', selector);
      page.getByRole = (role, spec) => loc(role, spec?.name);
      page.getByLabel = label => loc('label', label);
      page.getByText = text => loc('text', text);
      page.waitForResponse = predicate => new Promise(resolve => {listener = {predicate, resolve};});
      s.page = page; s.emit = emit;
      return {page, origin: 'http://127.0.0.1:19900', switchAM: async () => {}, close: async () => {
        if (s.closed) return; s.closed = true; actions.push({op: 'close', actor, isSettings, tablePage});
        if (options.lateWrite && !isSettings) emit('/api/v1/sandboxes/foreign', {}, {method: 'DELETE'});
        if (options.lateSeries && !measured && !isSettings) emit('/api/v1/' + (actor === 'admin' ? 'admin/' : '')
          + 'usage/metrics/reports/' + report.report_id + '/series', {});
        if (options.closeFailure) throw Error('close_failed');
      }};
    },
    evidence: async page => {
      const s = sessions.find(s => s.page === page); actions.push({op: 'evidence', closed: s.closed, actor: s.actor});
      if (s.closed && options.evidenceFailure) throw Error('evidence_failed');
      return {authentication: options.bearerOnly ? 'real_am_bearer' : 'browser_session', requests: structuredClone(s.receipts)};
    },
  };
  return {ctx, f, original, sessions, calls, actions, observations, controller, settings: () => settings,
    async cleanup() {
      const errors = [];
      for (const {run} of cleanups.toReversed()) {try {await run();} catch (error) {errors.push(error);}}
      return errors;
    }};
}
