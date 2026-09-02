import { describe, expect, it } from 'vitest';
import {
  compileSdkDocuments,
  SDK_DOCUMENT_FILENAMES,
  SDK_LANGUAGES,
  type RawSdkDocuments,
  type SdkLanguage,
} from './docSet';

function guide(order: number, label: string, sdk: SdkLanguage): string {
  return `# ${sdk} ${label}\n\n<!-- nav: ${String(order)} ${label} -->\n\nBody for ${label}.\n`;
}

function completeSources(): Record<string, string> {
  const sources: Record<string, string> = {};
  for (const sdk of SDK_LANGUAGES) {
    SDK_DOCUMENT_FILENAMES[sdk].forEach((filename, index) => {
      const label = filename === 'README.md' ? 'Overview' : filename.replace('.md', '');
      sources[`../../../sdk/${sdk}/docs/${filename}`] = guide((index + 1) * 10, label, sdk);
    });
  }
  return sources;
}

describe('compileSdkDocuments', () => {
  it('orders both complete guide sets by nav metadata and removes only that metadata', () => {
    const sources = completeSources();
    sources['../../../sdk/typescript/docs/README.md'] = guide(105, 'Overview', 'typescript');

    const compiled = compileSdkDocuments(sources);

    expect(compiled.typescript.at(-1)?.filename).toBe('README.md');
    expect(compiled.typescript).toHaveLength(10);
    expect(compiled.typescript.map((document) => document.filename)).toContain('sponsorships.md');
    expect(compiled.rust.map((document) => document.filename)).toEqual(SDK_DOCUMENT_FILENAMES.rust);
    expect(compiled.rust[0]?.markdown).toContain('# rust Overview');
    expect(compiled.rust[0]?.markdown).not.toContain('<!-- nav:');
  });

  it('rejects a missing guide', () => {
    const sources = completeSources();
    delete sources['../../../sdk/rust/docs/errors.md'];
    expect(() => compileSdkDocuments(sources)).toThrow(/rust guide set.*missing errors\.md/i);
  });

  it('rejects an unexpected guide', () => {
    const sources = completeSources();
    sources['../../../sdk/rust/docs/bonus.md'] = guide(90, 'Bonus', 'rust');
    expect(() => compileSdkDocuments(sources)).toThrow(/unexpected bonus\.md/i);
  });

  it('rejects malformed or misplaced nav metadata', () => {
    const malformed = completeSources();
    malformed['../../../sdk/typescript/docs/errors.md'] = '# Errors\n\n<!-- nav: nope Errors -->\n';
    expect(() => compileSdkDocuments(malformed)).toThrow(/malformed nav metadata/i);

    const misplaced = completeSources();
    misplaced['../../../sdk/typescript/docs/errors.md'] =
      '# Errors\n\nSome prose.\n\n<!-- nav: 70 Errors -->\n';
    expect(() => compileSdkDocuments(misplaced)).toThrow(/nav comment after its H1/i);
  });

  it('rejects a missing, repeated, or non-leading H1', () => {
    const missing = completeSources();
    missing['../../../sdk/rust/docs/sessions.md'] = '<!-- nav: 40 Sessions -->\n\nBody.\n';
    expect(() => compileSdkDocuments(missing)).toThrow(/must begin with its only H1/i);

    const repeated = completeSources();
    repeated['../../../sdk/rust/docs/sessions.md'] =
      '# Sessions\n\n<!-- nav: 40 Sessions -->\n\n# Again\n';
    expect(() => compileSdkDocuments(repeated)).toThrow(/must begin with its only H1/i);
  });

  it('rejects duplicate nav orders and labels', () => {
    const order = completeSources();
    order['../../../sdk/typescript/docs/errors.md'] = guide(60, 'Errors', 'typescript');
    expect(() => compileSdkDocuments(order)).toThrow(/duplicate nav order 60/i);

    const label = completeSources();
    label['../../../sdk/typescript/docs/errors.md'] = guide(95, 'Operations', 'typescript');
    expect(() => compileSdkDocuments(label)).toThrow(/duplicate nav label Operations/i);
  });

  it('rejects sources that are not raw strings or are outside the SDK glob', () => {
    const notRaw: RawSdkDocuments = {
      ...completeSources(),
      '../../../sdk/rust/docs/errors.md': { default: 'not raw' },
    };
    expect(() => compileSdkDocuments(notRaw)).toThrow(/not imported as raw text/i);

    expect(() =>
      compileSdkDocuments({ ...completeSources(), '../docs/elsewhere.md': '# Elsewhere' }),
    ).toThrow(/unexpected source path/i);
  });
});
