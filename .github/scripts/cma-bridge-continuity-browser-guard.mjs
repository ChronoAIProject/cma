// Isolated-page guard copied narrowly from old-browser-tab/hold.mjs, with
// passive fetch/SSE metadata observation. It never repairs or retries a request.
export function installReadOnlyObserver({ exactOrigin, credential, documentId, agentId }) {
  const nativeFetch = window.fetch.bind(window);
  const state = window.__cmaReadOnlyObserver = { documentId, events: [], blocked: [], dropped: 0, sequence: 0 };
  const push = (value) => {
    if (state.events.length < 5000) state.events.push({ at: new Date().toISOString(), ...value });
    else state.dropped++;
  };
  const blocked = (method, url) => {
    if (state.blocked.length < 100) state.blocked.push({ method, origin: url.origin, path: url.pathname });
    push({ type: 'blocked_mutation', method, path: url.pathname });
  };
  window.fetch = async (input, init) => {
    const request = new Request(input, init);
    const url = new URL(request.url, location.href);
    const method = request.method.toUpperCase();
    if (!['GET', 'HEAD', 'OPTIONS'].includes(method)
      || (url.origin === exactOrigin && url.pathname.startsWith('/auth/') && url.pathname !== '/auth/session')) {
      blocked(method, url);
      throw new TypeError('Read-only observer blocks mutations');
    }
    const id = ++state.sequence;
    const selected = url.origin === exactOrigin && (url.pathname === `/api/v2/agents/${agentId}/events`
      || (url.pathname === '/api/v1/agui/events' && url.searchParams.get('thread_id') === agentId));
    const headers = new Headers(request.headers);
    headers.delete('authorization');
    if (url.origin === exactOrigin && (url.pathname === '/auth/session' || url.pathname.startsWith('/api/'))) {
      headers.set('authorization', `Bearer ${credential}`);
    }
    push({ type: 'fetch_start', id, method, path: url.pathname, selected });
    let response;
    try { response = await nativeFetch(new Request(request, { headers, redirect: 'error' })); }
    catch (error) { push({ type: 'fetch_error', id, selected, errorName: error?.name ?? 'Error' }); throw error; }
    const sse = /^text\/event-stream(?:;|$)/i.test(response.headers.get('content-type') ?? '');
    push({ type: 'fetch_headers', id, selected, status: response.status, sse });
    if (!selected || !sse || !response.body) return response;
    try {
      const originalGetReader = response.body.getReader.bind(response.body);
      Object.defineProperty(response.body, 'getReader', { configurable: true, value: (...args) => {
        const reader = originalGetReader(...args);
        const originalRead = reader.read.bind(reader);
        const decoder = new TextDecoder();
        let buffer = '';
        let frames = 0;
        reader.read = async (...readArgs) => {
          let result;
          try { result = await originalRead(...readArgs); }
          catch (error) { push({ type: 'stream_read_error', id, selected, errorName: error?.name ?? 'Error', frames }); throw error; }
          if (result.done) { push({ type: 'stream_end', id, selected, frames }); return result; }
          // Inspect only the bytes the real application has already consumed.
          // Preserve the reader result and exceptions without teeing the body.
          try {
            buffer += decoder.decode(result.value, { stream: true });
            const complete = buffer.split(/\r?\n\r?\n/);
            buffer = complete.pop();
            for (const frame of complete) {
              frames++;
              if (frame.length > 65536) { push({ type: 'frame_observation_limit', id, selected }); continue; }
              const lines = frame.split(/\r?\n/);
              const data = lines.filter((line) => line.startsWith('data:')).map((line) => line.slice(5).trimStart()).join('\n');
              let value;
              try { value = JSON.parse(data); } catch { continue; }
              const eventName = lines.find((line) => line.startsWith('event:'))?.slice(6).trim();
              const type = eventName?.startsWith('cma.agent.') ? eventName : value.type;
              if (['cma.agent.error', 'cma.agent.live_gap', 'RUN_ERROR', 'RUN_STARTED', 'RUN_FINISHED'].includes(type)) {
                const code = value.data?.code ?? value.code;
                push({ type: 'stream_frame', id, selected, frameType: type,
                  code: typeof code === 'string' && /^[a-z0-9_]{1,80}$/.test(code) ? code : undefined,
                  reason: ['reconnected', 'lagged'].includes(value.reason) ? value.reason : undefined });
              }
            }
            if (buffer.length > 65536) { buffer = ''; push({ type: 'frame_observation_limit', id, selected }); }
          } catch { push({ type: 'stream_instrumentation_gap', id, selected }); }
          return result;
        };
        return reader;
      } });
    } catch { push({ type: 'stream_instrumentation_gap', id, selected }); }
    return response;
  };
}
