import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import { migrationShimOffences } from '../test/migrationShim';

const SRC = resolve(process.cwd(), 'src');
const sheet = (...parts: string[]) => join(SRC, ...parts);
const readSheet = (...parts: string[]) => readFileSync(sheet(...parts), 'utf8');

describe('the migrated session surfaces', () => {
  it('uses no migration shim beyond the shared responsive and canvas exemptions', () => {
    const sheets = [
      sheet('sessions', 'SessionRail.css'),
      sheet('sessions', 'SessionDrawer.css'),
      sheet('sessions', 'SessionHeader.css'),
      sheet('sessions', 'SessionActions.css'),
      sheet('sessions', 'NewSessionDialog.css'),
      sheet('sessions', 'StartupProgress.css'),
      sheet('sessions', 'BouncingText.css'),
      sheet('canvas', 'SessionCanvas.css'),
      sheet('canvas', 'ConversationRecap.css'),
    ];
    expect(
      migrationShimOffences(sheets, [
        '--focus-ring',
        '--focus-node',
        '--canvas-node-w',
        '--canvas-node-h',
        '--read-column',
        '--page-gutter',
      ]),
    ).toEqual([]);
  });

  it('keeps rail loading visible in both appearances and still on reduced motion', () => {
    const rail = readSheet('sessions', 'SessionRail.css');
    expect(rail).toMatch(
      /\.rail__ghost\s*\{[^}]*--rail-sweep: color-mix\(in srgb, var\(--dsw-alias-bg-base\) 65%, transparent\)[^}]*background: var\(--dsw-alias-bg-skeleton\)/,
    );
    expect(rail).toMatch(
      /body\[data-ds-dark-theme\] \.rail__ghost\s*\{[^}]*--rail-sweep: color-mix\(in srgb, var\(--dsw-alias-label-primary\) 5%, transparent\)/,
    );
    expect(rail).toMatch(
      /\.rail__ghost::after\s*\{[^}]*var\(--rail-sweep\)[^}]*animation: rail-sweep 2\.6s var\(--ds-ease-in-out\) infinite/,
    );
    expect(rail).toMatch(
      /@media \(prefers-reduced-motion: reduce\)[\s\S]*\.rail__ghost::after\s*\{[^}]*animation: none[^}]*content: none/,
    );
  });

  it('marks a plane choice with accent edge, wash, and a visible check', () => {
    const dialog = readSheet('sessions', 'NewSessionDialog.css');
    expect(dialog).toMatch(
      /\.planecard--chosen\s*\{[^}]*background: var\(--dsw-alias-interactive-bg-hover-accent\)[^}]*border-color: var\(--dsw-alias-state-business-primary\)/,
    );
    expect(dialog).toMatch(
      /\.planecard--chosen \.planecard__name::after\s*\{[^}]*color: var\(--dsw-alias-state-business-primary\)[^}]*content: '✓'/,
    );
  });

  it('uses the shared control scale and tokenized startup motion', () => {
    const header = readSheet('sessions', 'SessionHeader.css');
    const actions = readSheet('sessions', 'SessionActions.css');
    const startup = readSheet('sessions', 'StartupProgress.css');
    const bounce = readSheet('sessions', 'BouncingText.css');

    expect(header).toMatch(/\.sessionheader\s*\{[^}]*min-height: 52px/);
    expect(header).toMatch(
      /@media \(max-width: 29\.999rem\)[\s\S]*\.sessionheader\s*\{[^}]*flex-wrap: wrap/,
    );
    expect(actions).toMatch(/\.actions__more\s*\{[^}]*width: 32px/);
    expect(startup).toMatch(
      /\.startup__fill\s*\{[^}]*background: var\(--dsw-alias-state-business-primary\)[^}]*box-shadow: var\(--dsw-glow-accent\)[^}]*transition:\s*width var\(--ds-transition-duration\) var\(--ds-ease-in-out\)/,
    );
    expect(bounce).toMatch(/animation: bounce-char 1\.4s var\(--ds-ease-in-out\) infinite/);
  });

  it('keeps canvas focus accented and every terminal frame neutral', () => {
    const canvas = readSheet('canvas', 'SessionCanvas.css');
    const recap = readSheet('canvas', 'ConversationRecap.css');
    expect(canvas).toMatch(/\.node:hover\s*\{\s*border-color: var\(--dsw-alias-border-l2\);\s*\}/);
    expect(canvas).toMatch(
      /\.node\[data-focused='true'\]\s*\{[^}]*border-color: var\(--dsw-alias-state-business-primary\)[^}]*box-shadow: var\(--focus-node\)/,
    );
    expect(canvas).toMatch(
      /\.node__typinghere\s*\{[^}]*color: var\(--dsw-alias-state-business-primary\)[^}]*font-family: var\(--ds-font-family-code\)/,
    );

    const terminal = /\.node \.terminal\s*\{([^}]*)\}/.exec(canvas)?.[1] ?? '';
    expect(terminal).not.toMatch(/(?:background|color|filter|opacity):/);
    expect(recap).toMatch(
      /\.recap__dot\s*\{[^}]*background: var\(--dsw-alias-state-success-primary\)/,
    );
  });

  it('keeps the alias-tier account trigger and avatar treatments', () => {
    const profile = readSheet('profile', 'ProfileMenu.css');
    expect(profile).toMatch(
      /\.profile__trigger\[aria-expanded='true'\]\s*\{[^}]*background: var\(--dsw-alias-button-ghost-active-fill\)/,
    );
    expect(profile).toMatch(
      /\.profile__face\s*\{[^}]*background: var\(--dsw-alias-bg-layer-2\)[^}]*border: 1px solid var\(--dsw-alias-border-l1\)/,
    );
  });
});
