import { describe, expect, it } from 'vitest';
import { styleReferences } from './style-references.mjs';

const vendor = `
  .react-flow { --xy-ground-default: white; }
  .react-flow__background {
    background: var(--xy-ground, var(--xy-ground-default));
  }
`;

describe('computed-style scope inventory', () => {
  it('checks vendor fallback chains on Flow and keeps CMA aliases on body', () => {
    expect(
      styleReferences(
        `${vendor}
        .delivery-flow .react-flow { --xy-ground: var(--dsw-alias-bg-layer-2); }
        body { color: var(--ink, black); }`,
        vendor,
      ),
    ).toEqual({
      body: ['--dsw-alias-bg-layer-2', '--ink'],
      flow: ['--xy-ground', '--xy-ground-default'],
    });
  });

  it('does not exempt unknown vendor-looking names or missing CMA aliases', () => {
    expect(
      styleReferences('.react-flow { background: var(--xy-typo, var(--dsw-missing)); }', vendor)
        .body,
    ).toEqual(['--dsw-missing', '--xy-typo']);
  });

  it('retains the body requirement for a vendor name also used outside Flow', () => {
    const css = `${vendor} .other { color: var(--xy-ground); }`;
    expect(styleReferences(css, vendor)).toEqual({
      body: ['--xy-ground'],
      flow: ['--xy-ground-default'],
    });
  });

  it('does not classify mixed selectors or similarly named classes as Flow', () => {
    const css = `
      .react-flow, body { color: var(--xy-ground); }
      .react-flow-imposter { color: var(--xy-ground-default); }
      .react-flow + .other { color: var(--xy-ground); }
    `;
    expect(styleReferences(css, vendor)).toEqual({
      body: ['--xy-ground', '--xy-ground-default'],
      flow: [],
    });
  });

  it('retains references outside declarations in the complete body inventory', () => {
    expect(styleReferences(`${vendor} @supports (color: var(--xy-ground)) {}`, vendor)).toEqual({
      body: ['--xy-ground'],
      flow: ['--xy-ground-default'],
    });
  });
});
