import type { IDisposable, Terminal } from '@xterm/xterm';

/**
 * Escape-sequence hardening, per docs/Web_Style.md section 8 and issue #18.
 *
 * The byte stream reaching this terminal is produced by a coding agent running
 * the user's own untrusted code. Escape sequences are the attack surface.
 *
 * Three properties of xterm.js 6.0.0 shape this module, all verified against
 * the shipped source rather than assumed:
 *
 * 1. Registering a handler that returns `true` is the ONLY supported way to
 *    neutralise a sequence. IParser exposes no clearOscHandler and no way to
 *    remove or override a built-in.
 *
 * 2. Handler order is load-bearing. OscParser iterates its per-ident list
 *    most-recently-registered first and stops at the first `true`. A
 *    neutraliser therefore only wins if it is registered AFTER anything that
 *    would claim the same ident — so this must run immediately after
 *    `new Terminal()` and nothing registering OSC handlers may be loaded later.
 *
 * 3. A handler returning a Promise pauses the entire parser until it settles.
 *    Every neutraliser here is synchronous; returning a promise would convert
 *    a hostile output loop into a stall.
 */

/** Swallow the sequence. Synchronous by contract — see note 3 above. */
const SWALLOW = (): boolean => true;

/**
 * OSC idents neutralised, and why each one is here.
 *
 * Several are not implemented by xterm core today. They are registered anyway
 * so that adding an addon, or upgrading xterm, cannot quietly open the hole:
 * omission is the primary control, this is defence in depth.
 */
const NEUTRALISED_OSC: readonly { readonly ident: number; readonly reason: string }[] = [
  { ident: 0, reason: 'set icon name and window title' },
  { ident: 1, reason: 'set icon name' },
  { ident: 2, reason: 'set window title' },
  { ident: 4, reason: 'set ANSI palette entry — would let output rewrite the theme' },
  {
    ident: 7,
    reason: 'set working directory as a file:// URL — a leak sink in embedders that reflect it',
  },
  {
    ident: 8,
    reason: 'hyperlink; display text need not match its target, so a phishing primitive',
  },
  { ident: 9, reason: 'iTerm2 notification' },
  { ident: 10, reason: 'set/query foreground — query replies to the agent' },
  { ident: 11, reason: 'set/query background — query replies to the agent' },
  { ident: 12, reason: 'set/query cursor colour — query replies to the agent' },
  { ident: 52, reason: 'clipboard write AND read; the read form exfiltrates to the agent' },
  { ident: 104, reason: 'reset ANSI palette' },
  { ident: 110, reason: 'reset foreground' },
  { ident: 111, reason: 'reset background' },
  { ident: 112, reason: 'reset cursor colour' },
  { ident: 777, reason: 'urxvt extension dispatch, including notify' },
  { ident: 1337, reason: 'iTerm2 proprietary: file download, inline images, SetUserVar' },
];

/**
 * The colour sequences (4, 10, 11, 12 and their resets) are neutralised on
 * design grounds as well as security ones. Web_Style.md makes the 16-slot ANSI
 * palette authoritative; letting agent output redefine it would break that
 * contract and, at the extreme, let output paint foreground onto background
 * and render the session unreadable.
 */

/**
 * Apply hardening to a terminal.
 *
 * Call this immediately after constructing the Terminal and before loading any
 * addon — see note 2 above.
 *
 * @returns a disposable that removes every registered handler.
 */
export function hardenTerminal(term: Terminal): IDisposable {
  const registered: IDisposable[] = NEUTRALISED_OSC.map(({ ident }) =>
    term.parser.registerOscHandler(ident, SWALLOW),
  );

  return {
    dispose(): void {
      for (const handler of registered) {
        handler.dispose();
      }
      registered.length = 0;
    },
  };
}

/** The idents this module neutralises, exported so tests can assert coverage. */
export const NEUTRALISED_OSC_IDENTS: readonly number[] = NEUTRALISED_OSC.map((o) => o.ident);

/**
 * Scrollback cap. Each line costs cols * 12 bytes of cell data plus per-line
 * overhead, so 5000 lines is roughly 7MB at 120 columns and 18MB at 300 — big
 * enough that an agent transcript is not truncated, small enough that it
 * cannot exhaust the tab. Agent TUIs largely run on the alternate buffer,
 * which has no scrollback at all.
 */
export const SCROLLBACK_LINES = 5000;

/**
 * Flow-control watermarks for the write queue.
 *
 * Capping scrollback bounds the BUFFER, not the write QUEUE. xterm's
 * WriteBuffer throws synchronously once ~50MB is pending and undrained, so an
 * agent out-producing the parser raises inside the socket's message handler.
 * Callers pause their stream above HIGH and resume below LOW.
 */
export const WRITE_HIGH_WATERMARK = 100_000;
export const WRITE_LOW_WATERMARK = 10_000;
