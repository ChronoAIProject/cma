import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { render, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { EventGatewayDocs, GATEWAY_DOCUMENTS, gatewayDocLink } from './EventGatewayDocs';
import { GATEWAY_DIAGRAM_SIZES } from './gatewayDiagramSizes';

afterEach(() => vi.restoreAllMocks());

it('renders all three canonical documents with complete navigation and all reviewed diagrams', () => {
  const { container } = render(<EventGatewayDocs />);
  const download = container.querySelector<HTMLAnchorElement>(
    'a[download="agent-event-gateway.md"]',
  )!;
  const downloadHref = download.getAttribute('href');
  // jsdom's style selectors repeatedly read host, reparsing the entire Markdown
  // data URL. Reuse its real native result for this unchanged href in this test.
  const downloadHost = download.host;
  vi.spyOn(download, 'host', 'get').mockReturnValue(downloadHost);
  const article = container.querySelector('article')!;
  expect(article.querySelectorAll(':scope > section')).toHaveLength(3);
  for (const document of GATEWAY_DOCUMENTS) {
    const section = article.querySelector(`#${document.id}`)!;
    expect(section).toHaveAccessibleName(document.title);
    // Every source heading survives the port; code fences cannot create phantom headings.
    let fenced = false,
      headings = 0;
    for (const line of document.markdown.split('\n')) {
      if (line.startsWith('```')) fenced = !fenced;
      else if (!fenced && /^#{1,6} /.test(line)) headings++;
    }
    expect(section.querySelectorAll('h2,h3,h4,h5,h6')).toHaveLength(headings);
  }
  const ids = [...container.querySelectorAll('[id]')].map((element) => element.id);
  expect(new Set(ids).size).toBe(ids.length);
  for (const link of container.querySelectorAll<HTMLAnchorElement>('a[href^="#"]')) {
    expect(ids, `${link.textContent}: ${link.hash}`).toContain(link.hash.slice(1));
  }
  expect(article.querySelectorAll('img')).toHaveLength(12);
  for (const image of article.querySelectorAll('img')) {
    expect(image).toHaveAttribute('loading', 'lazy');
    expect(Number(image.getAttribute('width'))).toBeGreaterThan(0);
    expect(Number(image.getAttribute('height'))).toBeGreaterThan(0);
    expect(image.closest('a')).toHaveAttribute('href', image.getAttribute('src'));
  }
  expect(article.querySelector('code.language-mermaid')).toBeNull();
  expect(article.querySelector('a[href*="CMAEG_Operations"]')).toBeNull();
  expect(article.querySelector('a[href*="CMA_Public_APIs"]')).toBeNull();
  // Role enumeration repeatedly runs jsdom selectors against the full-document
  // data URL. Select native links directly, retaining name and visibility checks.
  const header = within(container.querySelector('header')!);
  const openApi = header.getByText('OpenAPI JSON', { selector: 'a[href]' });
  expect(openApi).not.toHaveAttribute('role');
  expect(openApi).toHaveAccessibleName('OpenAPI JSON');
  expect(openApi).toBeVisible();
  expect(openApi).toHaveAttribute('href', '/event-gateway/openapi.json');
  expect(header.getByText('Download document', { selector: 'a[href]' })).toBe(download);
  expect(download).not.toHaveAttribute('role');
  expect(download).toHaveAccessibleName('Download document');
  expect(download).toBeVisible();
  expect(download).toHaveAttribute('download', 'agent-event-gateway.md');
  const markdown = decodeURIComponent(download.getAttribute('href')!.split(',').slice(1).join(','));
  expect(markdown).not.toContain('](CMAEG_Protocol.md');
  for (const id of ids) expect(markdown).toContain(`<a id="${id}"></a>`);
  expect(markdown).toContain('```mermaid');
  expect(download).toHaveAttribute('href', downloadHref);
}, 15_000);

it('keeps every static diagram and reserved aspect ratio aligned with the backend source', () => {
  const backend = resolve(process.cwd(), '../api/crates/cmaeg');
  const manifest = readFileSync(
    resolve(backend, '../cmaeg-http/src/http/docs/diagrams.rs'),
    'utf8',
  );
  let diagramCount = 0;
  for (const document of GATEWAY_DOCUMENTS) {
    const diagrams = [...document.markdown.matchAll(/```mermaid\s*\n([\s\S]*?)\n```/g)];
    diagrams.forEach((match, index) => {
      const name = `${document.id}-${index}`;
      const hash = createHash('sha256').update(match[1]!.trim()).digest('hex');
      expect(manifest).toMatch(new RegExp(`"${hash}",\\s*"${name}"`));
      const svg = readFileSync(resolve(backend, `assets/docs/${name}.svg`), 'utf8');
      const dimensions = /viewBox="([^"]+)"/
        .exec(svg)![1]!
        .split(/\s+/)
        .slice(2)
        .map((value) => Math.ceil(Number(value)));
      expect(GATEWAY_DIAGRAM_SIZES[name]).toEqual(dimensions);
      diagramCount++;
    });
  }
  expect(diagramCount).toBe(12);
  expect(Object.keys(GATEWAY_DIAGRAM_SIZES)).toHaveLength(diagramCount);
});

it('keeps cross-document references in CMA and omits unavailable private relative links', () => {
  expect(gatewayDocLink('CMAEG_Protocol.md#5-management-api', 'integration')).toBe(
    '#protocol-5-management-api',
  );
  expect(gatewayDocLink('#custom-cloudevents', 'protocol')).toBe('#protocol-custom-cloudevents');
  expect(gatewayDocLink('CMAEG_Architecture.md', 'protocol')).toBe('#architecture');
  expect(gatewayDocLink('https://bot-gateway.chrono-ai.fun/docs#protocol', 'integration')).toBe(
    '#protocol',
  );
  expect(
    gatewayDocLink(
      'https://bot-gateway.chrono-ai.fun/docs/protocol#5-management-api',
      'integration',
    ),
  ).toBe('#protocol-5-management-api');
  expect(gatewayDocLink('https://example.test/reference', 'integration')).toBe(
    'https://example.test/reference',
  );
  expect(gatewayDocLink('CMAEG_Operations.md', 'protocol')).toBeUndefined();
  expect(gatewayDocLink('javascript:alert(1)', 'protocol')).toBeUndefined();
});

it('uses service discovery on the portal origin and preserves canonical document anchors', () => {
  for (const path of [
    '/openapi.json',
    '/event-gateway/openapi.json',
    'https://bot.chrono-ai.fun/event-gateway/openapi.json',
    'https://bot-gateway.chrono-ai.fun/openapi.json',
  ])
    expect(gatewayDocLink(path, 'protocol')).toBe('/event-gateway/openapi.json');
  expect(
    gatewayDocLink('https://bot.chrono-ai.fun/event-gateway/docs#protocol', 'integration'),
  ).toBe('#protocol');
  expect(
    gatewayDocLink(
      'https://bot.chrono-ai.fun/event-gateway/docs/protocol#5-management-api',
      'integration',
    ),
  ).toBe('#protocol-5-management-api');
  expect(
    gatewayDocLink('https://bot.chrono-ai.fun/docs/event-gateway#protocol', 'integration'),
  ).toBe('#protocol');
  const unrelated = 'https://bot-gateway.chrono-ai.fun/docs-other';
  expect(gatewayDocLink(unrelated, 'protocol')).toBe(unrelated);
});

it('rebases canonical Nyxid Gateway document links into the current portal', () => {
  expect(gatewayDocLink('https://cma.nyxid.ai/event-gateway/openapi.json', 'integration')).toBe(
    '/event-gateway/openapi.json',
  );
  expect(
    gatewayDocLink(
      'https://cma.nyxid.ai/event-gateway/docs/protocol#5-management-api',
      'integration',
    ),
  ).toBe('#protocol-5-management-api');
});
