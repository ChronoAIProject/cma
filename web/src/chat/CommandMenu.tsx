import { useEffect, useMemo, useState } from 'react';
import type { AvailableCommand } from '@chronoai/cma-sdk';
import './CommandMenu.css';

/**
 * The commands the kernel offers, reachable from the field — `Web_Style.md`
 * §18.4.
 *
 * They used to be printed as a static list at the foot of the message column,
 * below every turn: readable and unusable, so the reader retyped what they had
 * just read.
 *
 * Built from what the kernel advertised and from nothing else. This component
 * knows no command names and has no opinion about what any of them do — §18.2's
 * rule for a tool's kind, which §18.10 restates: a client that invented one
 * would be offering something the session cannot honour.
 *
 * The behaviour is §3's existing plate: arrows move, Enter or a click takes the
 * highlighted one, Escape closes and returns to the field. Reused rather than
 * reinvented so a reader who has met the command bar already knows this.
 */

interface CommandMenuProps {
  /** What the kernel said it offers. Empty or absent draws nothing. */
  readonly commands?: readonly AvailableCommand[];
  /** What has been typed after the `/`, which filters the list. */
  readonly query: string;
  /** Take this command. The caller inserts it and returns focus. */
  readonly onPick: (command: AvailableCommand) => void;
  /** Give up without taking one. */
  readonly onDismiss: () => void;
}

export function CommandMenu({ commands, query, onPick, onDismiss }: CommandMenuProps) {
  const matched = useMemo(
    () =>
      (commands ?? []).filter((command) =>
        command.name.toLowerCase().startsWith(query.toLowerCase()),
      ),
    [commands, query],
  );
  const [active, setActive] = useState(0);

  /*
   * Back to the top whenever the filter changes under the highlight.
   *
   * Adjusted during render rather than in an effect: React's own pattern for
   * a value derived from a changing input, and it costs one render instead of
   * two. An effect here re-renders after painting, so the highlight would be
   * visibly on the wrong row for a frame.
   */
  const [activeFor, setActiveFor] = useState(query);
  if (activeFor !== query) {
    setActiveFor(query);
    setActive(0);
  }

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      /*
       * `stopPropagation` as well as `preventDefault`, and this is the part
       * that matters: the field below listens for Enter too, and preventing
       * the default does nothing to stop the event reaching it. Without this,
       * choosing a command sent a turn instead.
       */
      const owned = () => {
        event.preventDefault();
        event.stopPropagation();
      };
      if (event.key === 'ArrowDown') {
        owned();
        setActive((at) => Math.min(at + 1, matched.length - 1));
      } else if (event.key === 'ArrowUp') {
        owned();
        setActive((at) => Math.max(at - 1, 0));
      } else if (event.key === 'Enter') {
        const picked = matched[active];
        if (picked !== undefined) {
          owned();
          onPick(picked);
        }
      } else if (event.key === 'Escape') {
        owned();
        onDismiss();
      }
    };
    /*
     * Captured, so the menu sees these before the field does.
     */
    window.addEventListener('keydown', onKey, true);
    return () => {
      window.removeEventListener('keydown', onKey, true);
    };
  }, [matched, active, onPick, onDismiss]);

  // A kernel that offers none opens nothing, on the same reasoning that keeps
  // an ungranted affordance off the surface entirely rather than drawing it
  // empty.
  if (matched.length === 0) {
    return null;
  }

  return (
    <ul className="cmdmenu" role="listbox" aria-label="Commands this session offers">
      {matched.map((command, at) => (
        <li key={command.name}>
          <button
            className="cmdmenu__item"
            type="button"
            role="option"
            aria-selected={at === active}
            data-active={at === active ? 'true' : undefined}
            // `mousedown` rather than `click`: the field is focused, and a
            // click would blur it first and close the menu under the pointer.
            onMouseDown={(event) => {
              event.preventDefault();
              onPick(command);
            }}
          >
            <span className="cmdmenu__name">{command.name}</span>
            <span className="cmdmenu__what">{command.description}</span>
          </button>
        </li>
      ))}
    </ul>
  );
}
