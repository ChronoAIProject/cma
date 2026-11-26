import { readFileSync, readdirSync } from 'node:fs';
import { basename, join, resolve } from 'node:path';
import { describe, expect, it } from 'vitest';

const BRAND = resolve(process.cwd(), 'src/assets/brand');
const PNG = join(BRAND, 'png');
const RETIRED_BLUE = new RegExp(['#2f', '5bea'].join(''), 'i');

function source(name: string) {
  return readFileSync(join(BRAND, name), 'utf8');
}

function documentFor(name: string) {
  return new DOMParser().parseFromString(source(name), 'image/svg+xml');
}

function markParts(document: Document) {
  return [...document.documentElement.children].filter(
    (element) => element.matches('path, rect') && !element.classList.contains('cma-sweep'),
  );
}

function paintIs(element: Element, ink: string) {
  return element.tagName === 'rect'
    ? element.getAttribute('fill') === ink
    : element.getAttribute('fill') === 'none' && element.getAttribute('stroke') === ink;
}

describe('the CMA mark', () => {
  it('keeps one five-part geometry in the still and animated files', () => {
    const still = markParts(documentFor('cma-mark.svg'));
    const animated = [
      ...documentFor('cma-mark-animated.svg').querySelectorAll(
        '.cma-arc, .cma-teeth, .cma-lens, .cma-bridge',
      ),
    ];
    const geometry = (element: Element) =>
      ['d', 'x', 'y', 'width', 'height', 'rx', 'stroke-width', 'stroke-linecap', 'stroke-linejoin']
        .map((attribute) => element.getAttribute(attribute))
        .join('|');

    expect(still).toHaveLength(5);
    expect(animated).toHaveLength(5);
    expect(animated.map(geometry)).toEqual(still.map(geometry));
    expect(still.every((element) => paintIs(element, 'currentColor'))).toBe(true);
    expect(animated.every((element) => paintIs(element, 'currentColor'))).toBe(true);
  });

  it('preserves the coupled draw, sweep, and reduced-motion timeline', () => {
    const animated = source('cma-mark-animated.svg');
    for (const declaration of [
      '1.1s cubic-bezier(0.65, 0, 0.35, 1)',
      'animation-delay: 0.72s',
      'animation-duration: 0.86s',
      '1.42s forwards',
      'animation-delay: 1.53s',
      '1.78s forwards',
      '2.4s infinite',
      '@media (prefers-reduced-motion: reduce)',
    ]) {
      expect(animated).toContain(declaration);
    }
  });

  it('matches the conversation mark inside its dark circular favicon tile', () => {
    const favicon = documentFor('cma-favicon.svg');
    const icon = favicon.querySelector('svg > svg');
    const parts = [...(icon?.children ?? [])];
    expect(parts).toHaveLength(5);
    expect(parts.every((element) => paintIs(element, 'currentColor'))).toBe(true);
    expect(icon?.getAttribute('color')).toBe('#c3fd43');
    expect([
      icon?.getAttribute('x'),
      icon?.getAttribute('y'),
      icon?.getAttribute('width'),
      icon?.getAttribute('height'),
    ]).toEqual(['0', '0', '32', '32']);
    expect(icon?.getAttribute('viewBox')).toBe('12 7.25 76 76');
    expect(favicon.querySelector('circle')?.getAttribute('fill')).toBe('#18181b');
    expect(favicon.documentElement.getAttribute('viewBox')).toBe('0 0 32 32');
    expect(parts.map((part) => part.outerHTML)).toEqual(
      markParts(documentFor('cma-mark.svg')).map((part) => part.outerHTML),
    );
  });
});

describe('the fixed brand exports', () => {
  it('uses the prescribed app-icon tile and export inks', () => {
    const appIcon = documentFor('cma-appicon.svg');
    expect(appIcon.documentElement.querySelector(':scope > rect')?.getAttribute('fill')).toBe(
      '#060606',
    );
    expect(
      [...appIcon.querySelectorAll('g path, g rect')].every((part) => paintIs(part, '#C3FD43')),
    ).toBe(true);
    expect(
      markParts(documentFor('cma-mark-ink.svg')).every((part) => paintIs(part, '#15283C')),
    ).toBe(true);
  });

  it('sets lockups in the shipped Archivo and IBM Plex Mono faces', () => {
    for (const file of [
      'cma-lockup-horizontal.svg',
      'cma-lockup-horizontal-ink.svg',
      'cma-lockup-horizontal-white.svg',
      'cma-lockup-stacked.svg',
    ]) {
      const lockup = documentFor(file);
      const text = [...lockup.querySelectorAll('text')];
      expect(text[0]?.getAttribute('font-family')).toBe(
        "'Archivo', 'Helvetica Neue', Arial, sans-serif",
      );
      expect(text[1]?.getAttribute('font-family')).toContain("'IBM Plex Mono'");
    }
  });

  it('leaves no retired blue in a source asset', () => {
    const offenders = readdirSync(BRAND)
      .filter((file) => file.endsWith('.svg'))
      .filter((file) => RETIRED_BLUE.test(source(file)));
    expect(offenders).toEqual([]);
  });
});

describe('the raster export set', () => {
  const dimensions: Readonly<Record<string, readonly [number, number]>> = {
    'appicon-1024.png': [1024, 1024],
    'appicon-180.png': [180, 180],
    'appicon-512.png': [512, 512],
    'favicon-16.png': [16, 16],
    'favicon-180.png': [180, 180],
    'favicon-32.png': [32, 32],
    'favicon-48.png': [48, 48],
    'favicon-512.png': [512, 512],
    'favicon-64.png': [64, 64],
    'lockup-horizontal-white.png': [968, 416],
    'lockup-horizontal.png': [968, 416],
    'lockup-stacked.png': [732, 736],
    'mark-1024.png': [1024, 1024],
    'mark-ink-1024.png': [1024, 1024],
    'mark-white-1024.png': [1024, 1024],
  };

  it('contains exactly the fifteen named PNGs at their promised pixel sizes', () => {
    const files = readdirSync(PNG)
      .filter((file) => file.endsWith('.png'))
      .sort();
    expect(files).toEqual(Object.keys(dimensions).sort());

    for (const file of files) {
      const png = readFileSync(join(PNG, file));
      expect([...png.subarray(0, 8)], basename(file)).toEqual([137, 80, 78, 71, 13, 10, 26, 10]);
      expect([png.readUInt32BE(16), png.readUInt32BE(20)], basename(file)).toEqual(
        dimensions[file],
      );
    }
  });
});

describe('browser chrome metadata', () => {
  it('ships OS-scheme colours and keeps the SVG after the Safari fallback', () => {
    const html = readFileSync(resolve(process.cwd(), 'index.html'), 'utf8');
    const document = new DOMParser().parseFromString(html, 'text/html');
    const colours = [...document.querySelectorAll('meta[name="theme-color"]')].map((meta) => [
      meta.getAttribute('media'),
      meta.getAttribute('content'),
    ]);
    expect(colours).toEqual([
      ['(prefers-color-scheme: light)', '#FAF2E0'],
      ['(prefers-color-scheme: dark)', '#060606'],
    ]);

    const icons = [...document.querySelectorAll('link[rel="icon"]')].map((link) =>
      link.getAttribute('href'),
    );
    expect(icons).toEqual([
      './src/assets/brand/pixel/favicon-16.png?no-inline',
      './src/assets/brand/pixel/favicon-32.png?no-inline',
      './src/assets/brand/pixel/favicon-48.png?no-inline',
      './src/assets/brand/pixel/favicon.svg?no-inline',
    ]);
    expect(document.querySelector('link[rel="apple-touch-icon"]')?.getAttribute('href')).toBe(
      './src/assets/brand/pixel/apple-touch-icon.png?no-inline',
    );
  });
});
