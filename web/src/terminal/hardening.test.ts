import { Terminal } from '@xterm/xterm';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { NEUTRALISED_OSC_IDENTS, SCROLLBACK_LINES, hardenTerminal } from './hardening';

/**
 * Issue #18's Definition of Done requires that OSC 52 from the session cannot
 * modify the host clipboard, asserted by test.
 *
 * A naive version of that assertion is vacuous: xterm 6 core does not
 * implement OSC 52 at all, so writing the sequence to an UNhardened terminal
 * also does nothing. These tests are therefore written to prove the property
 * that actually protects us — that our neutraliser INTERCEPTS the sequence and
 * beats any other handler registered for the same ident — rather than proving
 * only that today's core happens not to implement it.
 */

const terminals: Terminal[] = [];

function makeTerminal(): Terminal {
  const term = new Terminal({ cols: 80, rows: 24, scrollback: SCROLLBACK_LINES });
  terminals.push(term);
  return term;
}

/** term.write's callback is the only deterministic flush point. */
function write(term: Terminal, data: string): Promise<void> {
  return new Promise((resolve) => {
    term.write(data, resolve);
  });
}

afterEach(() => {
  for (const term of terminals) {
    term.dispose();
  }
  terminals.length = 0;
});

describe('hardenTerminal', () => {
  it('intercepts OSC 52 so a clipboard handler never runs', async () => {
    const term = makeTerminal();

    // Stands in for @xterm/addon-clipboard, which is banned from the codebase
    // but which an upgrade or a careless PR could reintroduce. Registered
    // BEFORE hardening, which is the case hardening must win.
    const clipboard = vi.fn(() => true);
    term.parser.registerOscHandler(52, clipboard);

    hardenTerminal(term);

    const payload = btoa('pwned');
    await write(term, `\x1b]52;c;${payload}\x07`);
    await write(term, `\x1b]52;p;${payload}\x1b\\`);
    await write(term, `\x1b]52;c;?\x07`);

    expect(clipboard).not.toHaveBeenCalled();
  });

  it('never echoes OSC 52 back to the agent', async () => {
    const term = makeTerminal();
    hardenTerminal(term);

    // The read form is the dangerous direction: it would send host clipboard
    // contents down onData, straight to the sandboxed agent.
    const toAgent: string[] = [];
    term.onData((d) => toAgent.push(d));

    await write(term, '\x1b]52;c;?\x07');
    await new Promise((r) => setTimeout(r, 0));

    expect(toAgent).toEqual([]);
  });

  it('stops escape sequences setting the window title', async () => {
    const term = makeTerminal();
    hardenTerminal(term);

    // Non-vacuous: xterm core DOES implement OSC 0/1/2 and would fire this.
    const titles: string[] = [];
    term.onTitleChange((t) => titles.push(t));

    await write(term, '\x1b]0;evil\x07');
    await write(term, '\x1b]1;evil\x07');
    await write(term, '\x1b]2;evil\x07');

    expect(titles).toEqual([]);
  });

  it('leaves the document title alone', async () => {
    const term = makeTerminal();
    hardenTerminal(term);
    const before = document.title;

    await write(term, '\x1b]2;hijacked\x07');

    expect(document.title).toBe(before);
  });

  it('intercepts palette manipulation so output cannot rewrite the theme', async () => {
    const term = makeTerminal();
    const palette = vi.fn(() => true);
    for (const ident of [4, 10, 11, 12, 104, 110, 111, 112]) {
      term.parser.registerOscHandler(ident, palette);
    }

    hardenTerminal(term);

    await write(term, '\x1b]11;#ff0000\x07');
    await write(term, '\x1b]4;1;#000000\x07');
    await write(term, '\x1b]104\x07');

    expect(palette).not.toHaveBeenCalled();
  });

  it('covers every sequence it claims to cover', () => {
    // Guards the list against a silent deletion during a refactor.
    for (const ident of [0, 1, 2, 4, 7, 8, 9, 52, 104, 777, 1337]) {
      expect(NEUTRALISED_OSC_IDENTS).toContain(ident);
    }
  });

  it('releases every handler on dispose', async () => {
    const term = makeTerminal();
    const underneath = vi.fn(() => true);
    term.parser.registerOscHandler(52, underneath);

    const hardening = hardenTerminal(term);
    hardening.dispose();

    await write(term, '\x1b]52;c;AAAA\x07');

    // Proves the neutralisers were genuinely registered and genuinely removed,
    // rather than dispose() being a no-op that silently leaks handlers.
    expect(underneath).toHaveBeenCalled();
  });

  it('caps the buffer so unbounded output cannot exhaust the tab', async () => {
    const term = new Terminal({ cols: 80, rows: 24, scrollback: 100 });
    terminals.push(term);

    await write(term, 'line\r\n'.repeat(5000));

    expect(term.buffer.active.length).toBeLessThanOrEqual(100 + 24);
  });
});
