import type { Arrangement } from './arrangement';
import './ArrangementToggle.css';

/**
 * Which way the session list is arranged — docs/Web_Style.md §1 and §13.
 *
 * An **arrangement, not a view.** §1 admits exactly two views, sessions and
 * operations, and says an arrangement "changes how the middle column is drawn
 * and nothing else". Calling this a view mode put a third thing in a
 * vocabulary that has room for two, which is why it used to sit in the app bar
 * beside the account control instead of with the list it rearranges.
 *
 * **It does not stop at the browser.** A terminal declares its own
 * measurements to the runtime drawing into it, so putting every attached
 * session in a 420×280 node asks each of them for roughly 47 columns and each
 * agent redraws at that width. Output already wrapped at 47 columns stays
 * wrapped at 47 columns, on the canvas and in the transcript afterwards. §13
 * records that as a real cost of this arrangement, and it may not be described
 * as a display preference that reaches nothing.
 *
 * Two quiet buttons rather than a switch: a switch implies on and off, and
 * neither arrangement is the absence of the other. `aria-pressed` carries the
 * state, so the accent is reinforcement rather than the only signal.
 *
 * Pressing the arrangement already showing is deliberately not a toggle.
 * Someone who cannot tell which one they are in reaches for the label of the
 * one they want, and flipping them away from it would be the opposite of what
 * they asked for.
 */

interface ArrangementToggleProps {
  readonly arrangement: Arrangement;
  readonly onChange: (arrangement: Arrangement) => void;
  /**
   * Whether the window has room for the canvas — §16.
   *
   * Offered at 60rem and above only: a grid of terminals in a narrow window is
   * several terminals nobody can read. Below that the option is **disabled
   * with its reason** rather than removed, per §7 and §11.
   */
  readonly canvasAvailable?: boolean;
}

const ARRANGEMENTS: readonly { readonly value: Arrangement; readonly label: string }[] = [
  { value: 'tabs', label: 'Tabs' },
  { value: 'canvas', label: 'Canvas' },
];

const NO_ROOM = 'Needs a window at least 60rem wide';

export function ArrangementToggle({
  arrangement,
  onChange,
  canvasAvailable = true,
}: ArrangementToggleProps) {
  const reasonId = 'arrangement-canvas-reason';
  return (
    <div className="arrangement" role="group" aria-label="Arrangement">
      {ARRANGEMENTS.map((option) => {
        const unavailable = option.value === 'canvas' && !canvasAvailable;
        return (
          <button
            key={option.value}
            type="button"
            className="arrangement__option"
            aria-pressed={arrangement === option.value}
            disabled={unavailable}
            aria-describedby={unavailable ? reasonId : undefined}
            onClick={() => {
              onChange(option.value);
            }}
          >
            {option.label}
          </button>
        );
      })}
      {/*
       * The reason, on the surface rather than in a tooltip (§7). The band it
       * sits in is narrow, so it is drawn only when it is true.
       */}
      {canvasAvailable ? null : (
        <span className="band__value arrangement__reason" id={reasonId}>
          {NO_ROOM}
        </span>
      )}
    </div>
  );
}
