import { createServer } from 'node:http';

export const webSha = '1'.repeat(40);
export const managerSha = '2'.repeat(40);
export const subject = 'synthetic-owner';
export const reportId = 'umr_' + '1'.repeat(32);
const base = '/api/v1/admin/usage/metrics';

function id(prefix, number) {
  return prefix + '_' + number.toString(16).padStart(32, '0');
}

function json(response, status, body) {
  response.writeHead(status, { 'Content-Type': 'application/json' });
  response.end(JSON.stringify(body));
}

function html(response, body) {
  response.writeHead(200, { 'Content-Type': 'text/html' });
  response.end(body);
}

function agentPage(response, agent) {
  html(
    response,
    `
    <section class="agent-surface__conversation"><p>Recorded turn</p></section>
    <textarea aria-label="Message"></textarea>
    <button aria-label="Send message" disabled>Send</button>
    <button aria-label="Stop response" hidden>Stop</button>
    <script>
      const input = document.querySelector('textarea');
      const [send, stop] = document.querySelectorAll('button');
      const path = '/api/v2/agents/${agent}/responses';
      input.addEventListener('input', () => { send.disabled = !input.value; });
      send.addEventListener('click', async () => {
        const message = input.value;
        const accepted = await fetch(path, { method: 'POST',
          headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ input: message }) });
        if (!accepted.ok) return;
        const marker = document.createElement('p');
        marker.textContent = message;
        document.querySelector('section').append(marker);
        input.value = '';
        stop.hidden = false;
      });
      stop.addEventListener('click', async () => {
        const accepted = await fetch(path + '/response-fixture/controls', { method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ command: { type: 'cancel' } }) });
        if (accepted.ok) stop.hidden = true;
      });
    </script>
  `,
  );
}

function operationsPage(response) {
  html(
    response,
    `
    <div aria-label="Operations resource dashboard" data-usage-requested-range="7d" data-usage-state="pending">
      <section aria-label="Allocated capacity now">Capacity</section>
      <section aria-label="Usage inventory"></section>
    </div>
    <script>
      fetch('${base}/initial/view', { method: 'POST' }).then(response => response.json()).then(() => {
        document.querySelector('[aria-label="Operations resource dashboard"]').dataset.usageState = 'committed';
        document.querySelector('[aria-label="Usage inventory"]').textContent = 'No Sandboxes match this selection.';
      });
    </script>
  `,
  );
}

export async function startFixture({ plainReturnsDesktop = false } = {}) {
  const fixtures = new Map();
  const counts = { plain: 0, desktop: 0, sends: 0, cancels: 0, reports: 0, deletes: 0 };
  let next = 1;
  let maxActive = 0;
  let populated = false;
  const server = createServer(async (request, response) => {
    const { pathname } = new URL(request.url, 'http://127.0.0.1');
    const method = request.method;
    if (pathname === '/build-info.json') return json(response, 200, { revision: webSha });
    if (pathname === '/api/v1/build-info') return json(response, 200, { source_sha: managerSha });
    if (pathname === '/auth/session') {
      return json(
        response,
        200,
        request.headers.cookie
          ? { authenticated: true, visitor: false, subject, capabilities: ['read_all_sandboxes'] }
          : { authenticated: false },
      );
    }

    if (pathname === '/api/v1/agents' && method === 'POST') {
      let payload = '';
      for await (const chunk of request) payload += chunk;
      const { machine_included: desktop } = JSON.parse(payload);
      const index = next++;
      const record = {
        agent: id('agt', index),
        sandbox: id('sbx', index),
        desktop: desktop || plainReturnsDesktop,
        state: 'ready',
      };
      if (desktop) counts.desktop += 1;
      else counts.plain += 1;
      fixtures.set(id('op', index), record);
      maxActive = Math.max(maxActive, fixtures.size);
      populated = true;
      return json(response, 202, { id: id('op', index) });
    }
    if (pathname.startsWith('/api/v1/agent-lifecycle/')) {
      const operation = pathname.split('/').at(-1);
      const record = fixtures.get(operation);
      return json(
        response,
        record ? 200 : 404,
        record && { id: operation, agent_id: record.agent, status: 'succeeded' },
      );
    }
    if (pathname === '/operations' && method === 'GET') return operationsPage(response);
    if (pathname === base + '/initial/view' && method === 'POST') {
      counts.reports += 1;
      return json(response, 200, {
        view: { report: { report_id: reportId } },
        initial: { rows: { report_id: reportId, items: [] } },
      });
    }
    if (pathname === '/api/v2/commands/command-fixture') {
      const active = fixtures.values().next().value;
      return json(response, 200, {
        status: 'completed',
        receipt: { command_id: 'command-fixture', agent_id: active?.agent },
      });
    }
    for (const [operation, record] of fixtures) {
      const agent = '/api/v1/agents/' + record.agent;
      const sandbox = '/api/v1/sandboxes/' + record.sandbox;
      const responses = '/api/v2/agents/' + record.agent + '/responses';
      if (pathname === '/agents/' + record.agent && method === 'GET')
        return agentPage(response, record.agent);
      if (pathname === responses && method === 'POST') {
        counts.sends += 1;
        response.setHeader('x-cma-response-id', 'response-fixture');
        response.setHeader('x-cma-command-id', 'command-fixture');
        return json(response, 200, {});
      }
      if (pathname === responses + '/response-fixture/controls' && method === 'POST') {
        counts.cancels += 1;
        return json(response, 202, {
          target: { response_id: 'response-fixture' },
          control: { receipt: { agent_id: record.agent, command_id: 'command-fixture' } },
        });
      }
      if (pathname === agent + '/stop' && method === 'POST') {
        record.state = 'stopped';
        return json(response, 202, {});
      }
      if (pathname === agent && method === 'GET') {
        return json(response, 200, {
          id: record.agent,
          status: record.state,
          desktop: record.desktop,
          sandbox: { id: record.sandbox, status: 'active' },
        });
      }
      if (pathname === agent && method === 'DELETE') {
        counts.deletes += 1;
        record.state = 'deleted';
        return json(response, 202, {});
      }
      if (pathname === sandbox && method === 'DELETE') {
        counts.deletes += 1;
        fixtures.delete(operation);
        return json(response, 202, {});
      }
    }
    return json(response, 404, {});
  });
  await new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', resolve);
  });
  return {
    origin: 'http://127.0.0.1:' + server.address().port,
    counts,
    resources: () => ({ current: fixtures.size, maxActive, populated }),
    close: () =>
      new Promise((resolve, reject) =>
        server.close((error) => (error ? reject(error) : resolve())),
      ),
  };
}
