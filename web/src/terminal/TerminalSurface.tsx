import { useEffect, useRef } from 'react';
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

interface TerminalSurfaceProps {
  readonly driver: TerminalDriver;
  /** Accessible name, since a terminal is otherwise an unlabelled region. */
  readonly label: string;
}

export function TerminalSurface({ driver, label }: TerminalSurfaceProps) {
  const hostRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const host = hostRef.current;
    if (host === null) {
      return;
    }

    const term = new Terminal({
      fontFamily: "ui-monospace, 'SF Mono', SFMono-Regular, Menlo, Consolas, monospace",
      fontSize: 14,
      lineHeight: 1.35,
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
    };
  }, [driver]);

  return <div className="terminal" ref={hostRef} role="group" aria-label={label} />;
}
