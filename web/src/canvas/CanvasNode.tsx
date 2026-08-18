import { useCallback, useEffect, useMemo, useRef } from 'react';
import type { KeyboardEvent, PointerEvent } from 'react';
import { TerminalSurface } from '../terminal/TerminalSurface';
import type { TerminalHandle } from '../terminal/TerminalSurface';
import { ConversationSurface } from '../chat/ConversationSurface';
import { toCapabilities } from '../agui';
import type { TerminalDriver } from '../terminal/types';
import type { Session } from '../sessions/types';
import { labelFor } from '../sessions/types';
import { abilitiesFor } from '../sessions/availability';
import { tabLabel } from '../sessions/sessionTime';
import { MAX_LIVE_NODES } from './layout';
import { createInputGate } from './inputGate';

/**
 * One session on the canvas: a framed terminal with a handle above it.
 *
 * The handle, not the frame, is the keyboard citizen. A terminal already
 * swallows arrow keys — they are shell history — so node-to-node navigation
 * can only live on a control outside the terminal, and the handle is it.
 *
 * The node places the caret itself on every pointer gesture rather than
 * leaving it to the browser's default. Where the caret lands decides which
 * terminal the next keystroke reaches, and "wherever the browser felt like" is
 * not an acceptable answer to that question: Safari does not focus a button on
 * click at all, and a click inside a terminal is intercepted by xterm.
 *
 * A node is labelled by when its session started, in the same words the rail
 * uses, because the canvas and the rail are two views of one list and a session
 * that is called one thing in one of them and another thing in the other is two
 * sessions as far as anyone reading can tell.
 */

interface CanvasNodeProps {
  readonly session: Session;
  /** Null while the session has no screen to show; see sessions/live.ts. */
  readonly driver: TerminalDriver | null;
  readonly focused: boolean;
  /**
   * Whether this node holds a terminal; see layout.ts for the budget. An
   * unattached node keeps its driver — the driver simply never gets attached,
   * so it never opens a connection.
   */
  readonly attached: boolean;
  /** Position in the fleet, announced so a screen reader knows where it is. */
  readonly index: number;
  readonly total: number;
  readonly onFocus: () => void;
  readonly onKeyDown: (event: KeyboardEvent<HTMLElement>) => void;
  readonly registerHandle: (element: HTMLButtonElement | null) => void;
}

export function CanvasNode({
  session,
  driver,
  focused,
  attached,
  index,
  total,
  onFocus,
  onKeyDown,
  registerHandle,
}: CanvasNodeProps) {
  const handleRef = useRef<HTMLButtonElement | null>(null);
  const terminalRef = useRef<TerminalHandle>(null);

  /*
   * Focus alone never opens the keyboard path. docs/Web_Style.md §14 decides
   * whether a session may be typed into at all, and the canvas is not a place
   * that rule gets a second opinion: a provisioning, rotating, idle or ended
   * session stays read-only however plainly it is being looked at.
   */
  const can = abilitiesFor(session);
  const interactive = focused && can.interactive;

  /*
   * Built once per driver, and told about focus afterwards. Rebuilding the
   * wrapper when focus moved would change the driver identity the terminal
   * surface sees, and it would rebuild its xterm instance and lose the
   * scrollback every time somebody looked at another node.
   */
  const gate = useMemo(() => (driver === null ? null : createInputGate(driver)), [driver]);
  useEffect(() => {
    gate?.setOpen(interactive);
  }, [gate, interactive]);

  const setHandle = useCallback(
    (element: HTMLButtonElement | null) => {
      handleRef.current = element;
      registerHandle(element);
    },
    [registerHandle],
  );

  /** A gesture on the chrome: the node takes focus, the caret goes to the handle. */
  function focusHandle(event: PointerEvent<HTMLElement>) {
    /*
     * Default prevented because this component is placing the caret itself. A
     * mousedown whose target is not focusable otherwise blurs to the body, and
     * Safari does not focus a button on click at all — either way the caret
     * would end up somewhere the arrow keys cannot navigate from.
     */
    event.preventDefault();
    onFocus();
    handleRef.current?.focus();
  }

  /** A gesture on the terminal: the caret goes into it, so typing works at once. */
  function focusTerminal(event: PointerEvent<HTMLElement>) {
    onFocus();
    if (terminalRef.current === null) {
      // A node with no terminal has nothing to type into, so the frame behaves
      // like the chrome: take the caret deliberately rather than letting it
      // fall to the body when the gesture lands on plain text.
      event.preventDefault();
      handleRef.current?.focus();
      return;
    }
    /*
     * Deliberately not prevented: this gesture belongs to xterm, which cancels
     * it itself, starts a text selection with it and focuses its own input.
     * Focusing explicitly as well makes the outcome the same on every browser
     * rather than a consequence of xterm's internals.
     */
    terminalRef.current.focus();
  }

  const started = tabLabel(session.createdAt);
  const state = labelFor(session.lifecycle);
  const name = `Session ${started} on ${session.kernel}, ${state}`;

  return (
    <section
      className="node"
      data-node-id={session.id}
      data-focused={focused}
      aria-label={name}
      /*
       * React's onFocus is focusin, so this catches focus arriving by any route
       * this component did not initiate — Tab, or a script. Whatever the
       * browser focused, the node containing it is the focused node.
       */
      onFocus={onFocus}
    >
      <header className="node__head" onPointerDown={focusHandle}>
        <button
          type="button"
          ref={setHandle}
          className="node__handle"
          aria-pressed={focused}
          /* Roving tabindex: one stop for the whole canvas. */
          tabIndex={focused ? 0 : -1}
          onKeyDown={onKeyDown}
          /* Keyboard activation, which produces a click and no pointer event. */
          onClick={onFocus}
        >
          <span className="node__started tabular">{started}</span>
          <span className="node__meta">
            {session.kernel} · {state}
          </span>
          <span className="sr-only">
            , node {index + 1} of {total}
          </span>
        </button>
        {/*
         * Read-only is stated in words on the node itself, not only in the
         * details panel. On a canvas the panel describes one session out of
         * many, so a node that quietly ignores typing would have nothing
         * nearby saying why.
         */}
        {can.interactive ? null : <span className="node__readonly">read-only</span>}
      </header>

      {/*
       * Pointer down rather than click: the caret must move before the gesture
       * reaches the terminal, so the terminal it reaches is already the focused
       * one and the input gate is already open.
       */}
      <div className="node__body" onPointerDown={focusTerminal}>
        {/*
         * Branched on the gate rather than on the driver, though a gate exists
         * exactly when a driver does. It leaves no expression in which a
         * terminal could be handed the raw driver as a fallback, and the raw
         * driver is the one that would carry a keystroke out of a node that
         * does not have focus.
         */}
        {gate === null ? (
          <p className="node__note">{can.reason}</p>
        ) : !attached ? (
          <p className="node__note">
            Not attached. The canvas keeps at most {MAX_LIVE_NODES} terminals connected and holds
            the ones most recently worked in. Select this node to attach it.
          </p>
        ) : session.plane === 'conversation' ? (
          /*
           * §13's sharp edge: every listed session is a framed node with its
           * own surface, so a canvas of mixed sessions draws chats and
           * terminals side by side. The node must not assume a terminal — this
           * component and the input gate below it were both written when it
           * could.
           *
           * The gate is not wrapped around this one. It exists to stop a
           * keystroke reaching an unfocused terminal, and a conversation has
           * no keystroke path: what it sends is a turn, from a composer, on a
           * surface somebody has to be looking at to type into.
           */
          <ConversationSurface session={session} capabilities={toCapabilities(session)} />
        ) : (
          <TerminalSurface
            ref={terminalRef}
            driver={gate.driver}
            /* Distinct from the node's own name, so a screen reader moving
               through the node does not hear the same label twice. */
            label={`Session ${started} terminal`}
            interactive={interactive}
          />
        )}
      </div>
    </section>
  );
}
