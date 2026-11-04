// @vitest-environment node
import { readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';
import ts from 'typescript';
import { describe, expect, it } from 'vitest';
import { ApiError } from '@chronoai/cma-sdk';
import { refusalSentence, refusalSentences } from './errors';
import { progressDetail } from './steps';
import { signupSentence } from './needs';

const forbidden =
  /\b(launch|revision|digest|idempotency|reconcile|retained|capability|ceiling|checkpoint|reach|fence|userservice|mib|millis)\b|\b[a-z]+_[a-z_]+\b/gi;
const visibleAttributes = new Set([
  'alt',
  'aria-label',
  'title',
  'placeholder',
  'description',
  'label',
]);

function visibleLiterals(source: string): readonly string[] {
  const file = ts.createSourceFile(
    'copy.tsx',
    source,
    ts.ScriptTarget.Latest,
    true,
    ts.ScriptKind.TSX,
  );
  const found: string[] = [];
  const scan = (node: ts.Node, visible = false) => {
    if (ts.isJsxText(node)) found.push(node.text);
    if (ts.isJsxAttribute(node)) {
      const attribute = node.name.getText(file);
      if (node.initializer !== undefined && attribute !== 'className')
        scan(node.initializer, visibleAttributes.has(attribute));
      return;
    }
    const literal =
      ts.isStringLiteral(node) ||
      ts.isNoSubstitutionTemplateLiteral(node) ||
      ts.isTemplateHead(node) ||
      ts.isTemplateMiddle(node) ||
      ts.isTemplateTail(node);
    if (
      literal &&
      (ts.isCaseClause(node.parent) ||
        (ts.isBinaryExpression(node.parent) &&
          [
            ts.SyntaxKind.EqualsEqualsEqualsToken,
            ts.SyntaxKind.ExclamationEqualsEqualsToken,
            ts.SyntaxKind.InKeyword,
          ].includes(node.parent.operatorToken.kind)))
    )
      return;
    if (literal && (visible || /\s/.test(node.text))) {
      if (!ts.isImportDeclaration(node.parent) && !ts.isExportDeclaration(node.parent))
        found.push(node.text);
    }
    const childVisible = visible || ts.isJsxExpression(node);
    ts.forEachChild(node, (child) => scan(child, childVisible));
  };
  scan(file);
  return found;
}

function sources(directory: string): readonly string[] {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) =>
    entry.isDirectory()
      ? sources(join(directory, entry.name))
      : /\.(ts|tsx)$/.test(entry.name) && !entry.name.includes('.test.')
        ? [join(directory, entry.name)]
        : [],
  );
}

describe('public vocabulary', () => {
  it('keeps ordinary visitor and creation literals free of wire words', () => {
    const failures = ['src/public', 'src/create']
      .flatMap(sources)
      .flatMap((path) =>
        visibleLiterals(readFileSync(path, 'utf8')).flatMap((copy) =>
          [...copy.matchAll(forbidden)].map((match) => `${path}: ${match[0]} in ${copy.trim()}`),
        ),
      );
    expect(failures).toEqual([]);
  });

  it('detects JSX text, expression branches, accessible labels and imperative error sentences', () => {
    const values = visibleLiterals(
      'const token = "failed_terminal"; const error = "Launch failed"; <><p>{busy ? "retry_ready" : "Ready"}</p><p>retained</p><button aria-label="checkpoint" className="input wire_class" onClick={() => setError("Launch denied")} /></>',
    );
    expect(
      values.flatMap((value) => [...value.matchAll(forbidden)].map((match) => match[0])),
    ).toEqual(['Launch', 'retry_ready', 'retained', 'checkpoint', 'Launch']);
  });

  it.each(Object.entries(refusalSentences))(
    'maps typed refusal %s without exposing server prose',
    (code, sentence) => {
      expect(refusalSentence(new ApiError(409, 'unsafe Launch checkpoint', code))).toBe(sentence);
      expect([...sentence.matchAll(forbidden)]).toEqual([]);
    },
  );

  it('translates known progress details and never exposes unknown internal language', () => {
    expect(progressDetail('Launch work will be retried')).toBe('CMA will try this step again.');
    expect(progressDetail('checkpoint writer ready')).toBeUndefined();
    expect(progressDetail('Installing project software')).toBe('Installing project software');
    expect(progressDetail('')).toBeUndefined();
  });

  it('distinguishes invitation, open social/email and closed registration', () => {
    const base = {
      socialProviders: ['github', 'google', 'apple', 'unknown'],
      emailSignup: false,
      inviteCodeRequired: false,
      registerAvailable: true,
    };
    expect(signupSentence(base)).toContain('GitHub, Google, or Apple');
    expect(signupSentence({ ...base, inviteCodeRequired: true })).toContain('invitation code');
    expect(signupSentence({ ...base, registerAvailable: false })).toContain('not available');
    expect(signupSentence({ ...base, socialProviders: [], emailSignup: true })).toContain(
      'with email',
    );
  });
});
