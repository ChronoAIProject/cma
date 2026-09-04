import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';

const scriptPath = resolve(process.cwd(), 'public/embed/v1.js');

async function executeEmbed(): Promise<string> {
  const source = await readFile(scriptPath, 'utf8');
  Function(source)();
  return source;
}

function completeElement(): HTMLElement {
  const element = document.createElement('cma-trigger');
  element.setAttribute('trigger', 'pub_example');
  element.setAttribute('placement', 'plc_readme');
  element.setAttribute('label', 'open_in_cma');
  element.setAttribute('theme', 'auto');
  element.setAttribute('origin', 'https://cma.example');
  return element;
}

describe('CMA trigger embed', () => {
  test('renders the canonical linked badge without a network API call', async () => {
    const fetch = vi.spyOn(globalThis, 'fetch');
    const source = await executeEmbed();
    const element = completeElement();
    document.body.append(element);

    const anchor = element.querySelector('a');
    const image = element.querySelector('img');
    expect(anchor).toHaveAttribute('href', 'https://cma.example/t/pub_example/plc_readme');
    expect(image).toHaveAttribute('alt', 'Open in CMA');
    expect(image).toHaveAttribute(
      'src',
      'https://cma.example/api/v1/triggers/public/pub_example/badge.svg?label=open_in_cma&theme=auto&placement=plc_readme',
    );
    expect(fetch).not.toHaveBeenCalled();
    expect(new TextEncoder().encode(source).byteLength).toBeLessThanOrEqual(3_072);
    expect(source).not.toMatch(/\beval\s*\(|\bfetch\s*\(|\bdocument\.cookie\b/);
    fetch.mockRestore();
  });

  test('renders nothing until every valid attribute is present', async () => {
    await executeEmbed();
    const element = document.createElement('cma-trigger');
    document.body.append(element);
    expect(element).toBeEmptyDOMElement();

    for (const [name, value] of [
      ['trigger', 'pub_example'],
      ['placement', 'plc_readme'],
      ['label', 'open_in_cma'],
      ['theme', 'auto'],
      ['origin', 'javascript:alert(1)'],
    ] as const) {
      element.setAttribute(name, value);
    }
    expect(element).toBeEmptyDOMElement();
    element.setAttribute('origin', 'https://cma.example/path');
    expect(element).toBeEmptyDOMElement();
    element.setAttribute('origin', 'https://cma.example');
    expect(element.querySelector('a')).not.toBeNull();
    element.setAttribute('label', 'arbitrary_text');
    expect(element).toBeEmptyDOMElement();
  });

  test('upgrades existing and dynamically inserted annotated anchors', async () => {
    const existing = document.createElement('a');
    existing.dataset.cmaTrigger = 'pub_example';
    existing.dataset.cmaPlacement = 'plc_docs';
    existing.dataset.cmaLabel = 'try_it';
    existing.dataset.cmaTheme = 'dark';
    existing.dataset.cmaOrigin = 'https://cma.example';
    document.body.append(existing);
    await executeEmbed();
    expect(existing).toHaveAttribute('href', 'https://cma.example/t/pub_example/plc_docs');
    expect(existing.querySelector('img')).toHaveAttribute('alt', 'Try it');

    const dynamic = existing.cloneNode(false) as HTMLAnchorElement;
    document.body.append(dynamic);
    await Promise.resolve();
    expect(dynamic.querySelector('img')).toHaveAttribute('alt', 'Try it');
  });
});
