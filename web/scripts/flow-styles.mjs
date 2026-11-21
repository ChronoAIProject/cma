import { ready, visit } from './browser.mjs';

// Public read responses only: selecting this channel renders the production
// lazy Flow chunk, custom cards, SVG edges/handles and interactive controls.
const channel = {
  channel_id: 'style-channel',
  record_id: 'style-record',
  version: 1,
  provider: { slug: 'cma_codex', level: 'l2' },
  definition: {
    name: 'Style check channel',
    schema_version: 1,
    provider: { slug: 'cma_codex', binding_id: 'style-binding' },
    sources: [{ type: 'custom' }],
    partition: { version: 1, key: 'conversation_and_sender' },
    reply: { auto_final: true, sinks: [{ type: 'source' }] },
  },
  endpoints: {
    custom_event_url: 'https://gateway.example/events',
    nyxid_callback_url: 'https://gateway.example/callback',
  },
};

export function flowResponse(path) {
  const bodies = {
    '/api/event-gateway/auth/session': {
      authenticated: true,
      subject: 'style-check',
      permissions: ['cma-eg:admin'],
      expires_at: 9999999999,
    },
    '/api/event-gateway/v1/channel-capabilities': {},
    '/api/event-gateway/v1/channels': { items: [channel], next_cursor: null },
    '/api/event-gateway/v1/channels/style-channel': channel,
  };
  if (!Object.hasOwn(bodies, path)) return undefined;
  return { status: 200, contentType: 'application/json', body: JSON.stringify(bodies[path]) };
}

export async function flowStyles(page, port) {
  await page.emulateMediaFeatures([{ name: 'prefers-reduced-motion', value: 'reduce' }]);
  await visit(page, port, 'domcontentloaded', '/event-gateway');
  const channelRow = '.gateway-sidebar [data-node-id="channel:style-record"] .tree-row__label';
  await ready(page, channelRow, { label: 'style check channel' });
  await page.click(channelRow);
  const scope = '.channel-map .delivery-flow .react-flow';
  await ready(page, `${scope} .react-flow__controls-button`, { label: 'Flow controls' });
  // A horizontal SVG path has zero bounding-box height despite a visible stroke.
  await page.waitForSelector(`${scope} .react-flow__edge-path[d]`);
  const problems = [];
  for (const dark of [false, true]) {
    await page.mouse.move(0, 0);
    const failures = await page.evaluate(
      (selector, isDark) => {
        document.body.toggleAttribute('data-ds-dark-theme', isDark);
        const flow = document.querySelector(selector);
        if (!flow) return ['the selected channel has no Flow canvas'];
        const problems = [];
        // Normalize each CMA alias using the browser, including shadows. These
        // are expectations on a disposable probe, never overrides on the app.
        const probe = document.createElement('div');
        document.body.append(probe);
        const expected = (property, token) => {
          probe.style.cssText = '';
          probe.style.setProperty(property, `var(${token})`);
          return getComputedStyle(probe).getPropertyValue(property);
        };
        const checks = [
          ['', 'background-color', '--dsw-alias-bg-layer-2'],
          ['.react-flow__background', 'background-color', '--dsw-alias-bg-layer-2'],
          ['.react-flow__background-pattern.dots', 'fill', '--dsw-alias-border-l3'],
          ['.react-flow__edge-path', 'stroke', '--dsw-alias-label-tertiary'],
          ['.react-flow__handle', 'background-color', '--dsw-alias-bg-layer-1'],
          ['.react-flow__handle', 'border-top-color', '--dsw-alias-border-l3'],
          ['.react-flow__controls', 'box-shadow', '--dsw-shadow-lv1'],
          ['.react-flow__controls-button', 'background-color', '--dsw-alias-bg-layer-1'],
          ['.react-flow__controls-button', 'color', '--dsw-alias-label-primary'],
          [
            '.react-flow__controls-button:not(:last-child)',
            'border-bottom-color',
            '--dsw-alias-border-l2',
          ],
          [
            '.channel-map__node[data-kind="producer"]',
            'background-color',
            '--dsw-alias-bg-layer-1',
          ],
          ['.channel-map__node[data-kind="producer"]', 'color', '--dsw-alias-label-primary'],
        ];
        for (const [target, property, token] of checks) {
          const elements = target ? [...flow.querySelectorAll(target)] : [flow];
          if (elements.length === 0) problems.push(`${target}: nothing rendered`);
          const want = expected(property, token);
          for (const element of elements) {
            const actual = getComputedStyle(element).getPropertyValue(property);
            if (actual !== want)
              problems.push(
                `${target || '.react-flow'} ${property}: ${actual}; expected ${want} (${token})`,
              );
          }
        }
        for (const edge of flow.querySelectorAll('.react-flow__edge-path')) {
          if (getComputedStyle(edge).strokeWidth !== '1.5px' || !edge.getAttribute('d'))
            problems.push('Flow edge has no 1.5px rendered path');
        }
        if (flow.getBoundingClientRect().height < 320)
          problems.push('Flow canvas is shorter than its 320px minimum');
        probe.remove();
        return problems;
      },
      scope,
      dark,
    );
    problems.push(...failures.map((problem) => `Flow ${dark ? 'dark' : 'light'}: ${problem}`));
    await page.hover(`${scope} .react-flow__controls-button`);
    const hover = await page.evaluate((selector) => {
      const button = document.querySelector(`${selector} .react-flow__controls-button`);
      const probe = document.createElement('div');
      probe.style.backgroundColor = 'var(--dsw-alias-interactive-bg-hover)';
      document.body.append(probe);
      const actual = getComputedStyle(button).backgroundColor;
      const expected = getComputedStyle(probe).backgroundColor;
      probe.remove();
      return actual === expected ? [] : [`control hover: ${actual}; expected ${expected}`];
    }, scope);
    problems.push(...hover.map((problem) => `Flow ${dark ? 'dark' : 'light'}: ${problem}`));
  }
  await page.emulateMediaFeatures([]);
  return problems;
}
