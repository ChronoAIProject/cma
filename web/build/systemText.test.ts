import { readFileSync } from 'node:fs';
import postcss from 'postcss';
import { expect, it } from 'vitest';
import { systemText } from './systemText';

async function transform(css: string) {
  return (await postcss([systemText()]).process(css, { from: undefined })).css;
}

it('scales workspace font aliases while preserving unitless leading and nontext tokens', async () => {
  const tokens = readFileSync('src/styles/tokens.css', 'utf8');
  const result = postcss.parse(await transform(tokens));
  const values = new Map<string, string>();
  result.walkDecls((declaration) => {
    values.set(declaration.prop, declaration.value);
  });
  expect(values.get('--dsw-alias-workspace-body-font')).toBe(
    '400 calc(15px * var(--cma-system-font-scale, 1))/1.75 var(--dsw-font-family)',
  );
  expect(values.get('--dsw-alias-workspace-group-font')).toBe(
    '400 calc(11.5px * var(--cma-system-font-scale, 1))/calc(18px * var(--cma-system-font-scale, 1)) var(--dsw-font-family)',
  );
  expect(values.get('--dsw-alias-workspace-tool-font')).toBe(
    '400 calc(12.5px * var(--cma-system-font-scale, 1))/calc(20px * var(--cma-system-font-scale, 1)) var(--dsw-alias-workspace-mono)',
  );
  const nontext = (css: string) => {
    const declarations: string[] = [];
    postcss.parse(css).walkDecls((declaration) => {
      if (!/^--dsw-alias-.*-font$/.test(declaration.prop))
        declarations.push(declaration.toString());
    });
    return declarations;
  };
  expect(nontext(result.toString())).toEqual(nontext(tokens));
});

it('scales rem font aliases once and leaves consuming and inherited fonts alone', async () => {
  const css = `body {
    --dsw-alias-example-font: 600 1rem/1.5rem sans-serif;
    --dsw-alias-row-height: 36px;
  }
  .parent, .child { font: var(--dsw-alias-example-font); }
  .inherited { font: inherit; font-size: 1em; line-height: 1.75; padding: 16px; }`;
  const result = await transform(css);
  expect(result).toContain(
    '--dsw-alias-example-font: 600 calc(1rem * var(--cma-system-font-scale, 1))/calc(1.5rem * var(--cma-system-font-scale, 1)) sans-serif',
  );
  expect(result).toContain('--dsw-alias-row-height: 36px');
  expect(result).toContain('.parent, .child { font: var(--dsw-alias-example-font); }');
  expect(result).toContain(
    '.inherited { font: inherit; font-size: 1em; line-height: 1.75; padding: 16px; }',
  );
  expect(await transform(result)).toBe(result);
});

it('retains scaling at the consumer for primitive font tokens and literal shorthands', async () => {
  const result = await transform(`.heading { font: var(--dsw-font-markdown-h3); }
    .literal { font: 500 16px/24px sans-serif; width: 16px; }`);
  expect(result).toContain(
    'font-size: calc(var(--dsw-font-markdown-h3-font-size) * var(--cma-system-font-scale, 1))',
  );
  expect(result).toContain(
    'line-height: calc(var(--dsw-font-markdown-h3-line-height) * var(--cma-system-font-scale, 1))',
  );
  expect(result).toContain(
    'font: 500 calc(16px * var(--cma-system-font-scale, 1))/calc(24px * var(--cma-system-font-scale, 1)) sans-serif',
  );
  expect(result).toContain('width: 16px');
});
