import { useEffect, useImperativeHandle, useRef } from 'react';
import type { Ref } from 'react';
import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import '@xterm/xterm/css/xterm.css';
import { SCROLLBACK_LINES, hardenTerminal } from './hardening';
import { activeThemeName } from './theme';
import { resolveTheme, storedThemeId } from './themes';
import type { TerminalDriver, TerminalSink } from './types';
import './TerminalSurface.css';

/**
 * Hosts an xterm.js terminal and binds it to a driver.
 *
 * This component knows nothing about any concrete driver — that is the whole
 * point of the seam. It owns construction, hardening, geometry and teardown;
 * the driver owns content.
 */

/**
 * What a container may ask of a surface it does not own.
 *
 * Deliberately one method. A caller placing the caret is a legitimate need —
 * a canvas of terminals has to decide which of them a click lands in — whereas
 * reaching for the buffer or the driver from outside would dissolve the seam.
 */
export interface TerminalHandle {
  focus(): void;
}

/**
 * Apply the input mode to a live terminal.
 *
 * Shared by construction and by later changes so the two can never disagree —
 * a rebuilt terminal that kept its default tab stop would put a session nobody
 * is working in back in the tab order without anything in the diff saying so.
 */
function applyInputMode(term: Terminal, interactive: boolean): void {
  // xterm drops the data event entirely while this is set and marks its input
  // textarea read-only, so a surface that is not the focused one cannot
  // originate a keystroke even before the driver sees one.
  term.options.disableStdin = !interactive;
  if (term.textarea) {
    // Out of the tab order while not interactive, so Tab walks the containing
    // view rather than every terminal in it. Still click-focusable, which is
    // how a pointer moves focus between them.
    term.textarea.tabIndex = interactive ? 0 : -1;
  }
}

interface TerminalSurfaceProps {
  readonly driver: TerminalDriver;
  /** Accessible name, since a terminal is otherwise an unlabelled region. */
  readonly label: string;
  /**
   * Whether this surface may take keyboard input. Defaults to true, which is
   * every case where one terminal owns the view.
   *
   * A non-interactive surface stays attached and keeps rendering: the canvas
   * shows many sessions at once and only one of them may receive keys, and a
   * session that §14 says cannot be typed into is never interactive at all.
   */
  readonly interactive?: boolean;
  readonly ref?: Ref<TerminalHandle>;
}

export function TerminalSurface({ driver, label, interactive = true, ref }: TerminalSurfaceProps) {
  const hostRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<Terminal | null>(null);
  /*
   * Read at construction without joining the effect's dependency list. Putting
   * `interactive` in those dependencies would rebuild the whole terminal every
   * time focus moved, discarding the scrollback and re-attaching the driver.
   */
  const interactiveRef = useRef(interactive);

  useImperativeHandle(
    ref,
    () => ({
      focus() {
        // A no-op before the terminal exists, which is the frame between mount
        // and the construction effect. A caller cannot usefully act on that.
        termRef.current?.focus();
      },
    }),
    [],
  );

  useEffect(() => {
    const host = hostRef.current;
    if (host === null) {
      return;
    }

    const term = new Terminal({
      /*
       * The same stack the rest of the product's code takes, so a command and
       * its output line up whether they are read here or in a transcript.
       * The bare `monospace` tail is deliberate and is the one place it is
       * allowed: xterm needs a terminal font to exist even where none of the
       * named ones do, and a terminal with no font is not a terminal.
       */
      fontFamily:
        "'SF Mono', 'JetBrains Mono', 'Fira Code', Consolas, 'Liberation Mono', Menlo, monospace",
      fontSize: 13,
      // 22px at 13px, which is the line height the code blocks use.
      lineHeight: 22 / 13,
      letterSpacing: 0,
      scrollback: SCROLLBACK_LINES,
      cursorBlink: true,
      theme: resolveTheme(activeThemeName(), storedThemeId()).theme,
      /*
       * Explicit even though every field defaults off. XTWINOPS lets output
       * query and manipulate the window; stating it here means a later
       * "just enable resize reporting" edit shows up in a diff instead of
       * riding in on a default.
       */
      windowOptions: {},
    });

    /*
     * Immediately after construction and before any addon. xterm runs the most
     * recently registered OSC handler first, so hardening applied later than a
     * conflicting registration would silently lose.
     */
    const hardening = hardenTerminal(term);

    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(host);
    // After open(), which is what creates the textarea this touches.
    applyInputMode(term, interactiveRef.current);

    const sink: TerminalSink = {
      write(data) {
        try {
          term.write(data);
        } catch (error: unknown) {
          // xterm throws synchronously once ~50MB is pending and undrained.
          // #18 requires a real UI state here rather than a silent freeze.
          term.writeln('');
          term.writeln(
            `\x1b[31m[output discarded: ${
              error instanceof Error ? error.message : 'write buffer overflow'
            }]\x1b[0m`,
          );
        }
      },
      fail(message) {
        term.writeln('');
        term.writeln(`\x1b[31m[${message}]\x1b[0m`);
      },
      finish(reason) {
        term.writeln('');
        term.writeln(`\x1b[90m[${reason}]\x1b[0m`);
      },
    };

    const dataSub = term.onData((data) => {
      driver.send(data);
    });

    /**
     * fit() throws if the host has no layout yet, which is routine on first
     * paint and whenever a tab is hidden.
     */
    const safeFit = () => {
      if (host.clientWidth === 0 || host.clientHeight === 0) {
        return;
      }
      try {
        fit.fit();
      } catch {
        // A transient zero-size or detached host. The next observation refits.
      }
    };

    safeFit();
    // #18: geometry must reach the runtime before the first data frame.
    driver.resize(term.cols, term.rows);
    driver.attach(sink);

    const resizeSub = term.onResize(({ cols, rows }) => {
      driver.resize(cols, rows);
    });

    let frame = 0;
    const observer = new ResizeObserver(() => {
      // Coalesce to one fit per frame; ResizeObserver can fire per pixel
      // during a drag and each fit reflows the whole buffer.
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(safeFit);
    });
    observer.observe(host);
    termRef.current = term;

    /*
     * React 19 StrictMode runs this effect twice in development. Without a
     * complete teardown the first Terminal stays attached to the host, leaving
     * two canvases stacked and two driver subscriptions live.
     */
    return () => {
      cancelAnimationFrame(frame);
      observer.disconnect();
      resizeSub.dispose();
      dataSub.dispose();
      driver.detach();
      hardening.dispose();
      term.dispose();
      termRef.current = null;
    };
  }, [driver]);

  /*
   * Declared after the effect that builds the terminal, so on mount the
   * instance already exists by the time this runs.
   */
  useEffect(() => {
    interactiveRef.current = interactive;
    const term = termRef.current;
    if (term === null) {
      return;
    }

    applyInputMode(term, interactive);
    if (!interactive) {
      /*
       * Blur as well as disable. A read-only textarea still holds the caret,
       * and leaving one in a terminal that ignores input reads as a frozen
       * session rather than as one nobody is working in.
       */
      term.blur();
    }
  }, [interactive]);

  return <div className="terminal" ref={hostRef} role="group" aria-label={label} />;
}
