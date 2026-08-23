import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import { migrationShimOffences } from '../test/migrationShim';

const CHAT = resolve(process.cwd(), 'src', 'chat');
const sheet = (name: string) => resolve(CHAT, name);

describe('the migrated conversation sheets', () => {
  it('use no migration shim beyond the shared and responsive exemptions', () => {
    const sheets = [
      sheet('MessageStream.css'),
      sheet('Composer.css'),
      sheet('Thinking.css'),
      sheet('ApprovalCard.css'),
      sheet('SignInCard.css'),
    ];
    expect(
      migrationShimOffences(sheets, ['--focus-ring', '--read-column', '--page-gutter']),
    ).toEqual([]);
  });

  it('keeps the composer exception and gives waiting cards their named tones', () => {
    const composer = readFileSync(sheet('Composer.css'), 'utf8');
    const composerFocus = /\.composer:focus-within\s*\{([^}]*)\}/.exec(composer)?.[1] ?? '';
    expect(composerFocus).toContain('border-color: var(--dsw-alias-state-business-primary)');
    expect(composerFocus).not.toContain('box-shadow');

    const approval = readFileSync(sheet('ApprovalCard.css'), 'utf8');
    expect(approval).toMatch(
      /\.approval__eyebrow\s*\{[^}]*color: var\(--dsw-alias-state-warn-label\)[^}]*background: var\(--dsw-alias-state-warn-tertiary\)/,
    );

    const signIn = readFileSync(sheet('SignInCard.css'), 'utf8');
    expect(signIn).toMatch(
      /\.signin\s*\{[^}]*border: 1px solid var\(--dsw-alias-state-business-primary\)[^}]*box-shadow: var\(--dsw-glow-accent\)/,
    );
  });

  it('keeps reasoning borderless on hover and stops its sweep on request', () => {
    const thinking = readFileSync(sheet('Thinking.css'), 'utf8');
    const hover = /\.thinking__key:hover\s*\{([^}]*)\}/.exec(thinking)?.[1] ?? '';
    expect(hover).not.toContain('border');
    expect(thinking).toMatch(/color-mix\(in srgb, var\(--dsw-alias-bg-base\) 60%, transparent\)/);
    expect(thinking).toMatch(
      /@media \(prefers-reduced-motion: reduce\)[\s\S]*\.thinking__key::after\s*\{[^}]*animation: none[^}]*opacity: 0/,
    );
  });
});
