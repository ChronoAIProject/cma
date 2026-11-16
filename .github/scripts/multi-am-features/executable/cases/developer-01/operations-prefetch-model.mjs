// Offline interception model. It starts no browser or application process.
import {EventEmitter} from 'node:events';

export function withPrefetch(h, options = {}) {
  const adapter = h.ctx.fixture('browser'), open = adapter.open, evidence = adapter.evidence;
  const sessions = [], lookup = new WeakMap();
  adapter.open = async args => {
    const session = await open(args), {page} = session;
    if (!page.on) {
      const events = new EventEmitter();
      for (const name of ['on', 'off', 'emit', 'listenerCount']) page[name] = events[name].bind(events);
    }
    const saved = {receipts: [], page, emitted: false, finished: false}; sessions.push(saved); lookup.set(page, saved);
    const path = options.path ?? '/api/agent-manager/v1/admin/usage/metrics/reports';
    const method = options.method ?? 'POST', status = options.status ?? 202;
    const request = {method: () => method, url: () => session.origin + path,
      postDataJSON: () => {if (options.requestFailure) throw Error('request_body_failed'); return {from: 1, to: 2};}};
    const requestId = 'prefetch-' + sessions.length;
    const response = {request: () => request, status: () => status, url: request.url,
      headerValue: async () => options.unjoined ? 'unjoined-prefetch' : requestId,
      json: async () => {if (options.bodyFailure) throw Error('report_body_failed');
        return options.body ?? {report_id: requestId, status: status === 200 ? 'ready' : 'pending'};}};
    const finish = () => {
      if (saved.finished || options.unresolved) return; saved.finished = true;
      saved.receipts.push({method, status, request_id: requestId, am: args.am,
        serving_am: options.wrongAM ? (args.am === 'a' ? 'b' : 'a') : args.am,
        serving: {pod_uid: 'model'}, serving_after: {pod_uid: 'model'}, started_at: 'start', ended_at: 'end',
        attribution: {status: 'proven'}});
      if (options.duplicate) saved.receipts.push(structuredClone(saved.receipts[0]));
      page.emit('response', response);
    };
    const goto = page.goto;
    page.goto = async url => {
      await goto(url);
      if (new URL(url).searchParams.get('tab') !== 'settings' || saved.emitted) return;
      saved.emitted = true; page.emit('request', request);
      if (!options.late) finish();
    };
    return {...session, close: async () => {try {await session.close();} finally {if (saved.emitted) finish();}}};
  };
  adapter.evidence = async page => {
    const proof = await evidence(page);
    return {...proof, requests: [...proof.requests, ...lookup.get(page).receipts]};
  };
  return sessions;
}
