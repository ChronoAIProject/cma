import { readFileSync } from 'node:fs';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it, vi } from 'vitest';
import { CmaTriggerButton, type CmaTriggerButtonProps } from '../src/react.js';
import { ValidationError } from '../src/errors.js';

const descriptor = JSON.parse(
  readFileSync(
    new URL(
      '../../../api/crates/layer1-public/tests/fixtures/trigger-embed/descriptor.json',
      import.meta.url,
    ),
    'utf8',
  ),
) as { public_url: string; badge_url: string; label_text: string };

describe('SSR-safe React Trigger button', () => {
  it('renders the backend descriptor URLs without calling fetch or browser APIs', () => {
    const fetch = vi.spyOn(globalThis, 'fetch').mockRejectedValue(new Error('must not fetch'));
    try {
      const markup = renderToStaticMarkup(
        createElement(CmaTriggerButton, {
          triggerId: 'pub_example',
          placementId: 'plc_readme',
          baseUrl: 'https://cma.example',
        }),
      );
      expect(markup).toContain(`<a href="${descriptor.public_url}">`);
      expect(markup).toContain(`src="${descriptor.badge_url.replaceAll('&', '&amp;')}"`);
      expect(markup).toContain(`alt="${descriptor.label_text}"`);
      expect(markup).not.toMatch(/<script|onClick|\/launches/);
      expect(fetch).not.toHaveBeenCalled();
    } finally {
      fetch.mockRestore();
    }
  });

  it('supports optional placement and the public production default', () => {
    const markup = renderToStaticMarkup(
      createElement(CmaTriggerButton, { triggerId: 'pub_example' }),
    );
    expect(markup).toContain('href="https://cma.nyxid.ai/t/pub_example"');
    expect(markup).not.toContain('placement=');
    expect(markup).toContain('label=open_in_cma&amp;theme=auto');
  });

  it.each([
    ['open_in_cma', 'Open in CMA'],
    ['chat_with_this', 'Chat with this'],
    ['run_with_cma', 'Run with CMA'],
    ['try_it', 'Try it'],
  ] as const)('renders the closed label %s and selected theme', (label, alt) => {
    for (const theme of ['auto', 'light', 'dark'] as const) {
      const markup = renderToStaticMarkup(
        createElement(CmaTriggerButton, {
          triggerId: 'pub_example',
          label,
          theme,
          baseUrl: 'http://127.0.0.1:8080/',
        }),
      );
      expect(markup).toContain(`alt="${alt}"`);
      expect(markup).toContain(`label=${label}&amp;theme=${theme}`);
    }
  });

  it.each([
    { triggerId: '../escape' },
    { triggerId: '' },
    { triggerId: 'a'.repeat(129) },
    { placementId: '' },
    { placementId: 'path/injection' },
    { baseUrl: '//attacker.invalid' },
    { baseUrl: 'javascript:alert(1)' },
    { baseUrl: 'https://user@example.invalid' },
    { baseUrl: 'https://example.invalid/path' },
    { baseUrl: 'https://example.invalid?query' },
    { baseUrl: 'https://example.invalid#hash' },
    { label: 'invented' },
    { theme: 'invented' },
  ])('rejects invalid props before rendering: %j', (override) => {
    expect(() =>
      CmaTriggerButton({ triggerId: 'pub_example', ...override } as CmaTriggerButtonProps),
    ).toThrow(ValidationError);
  });
});
