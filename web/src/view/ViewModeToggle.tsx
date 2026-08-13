import type { ViewMode } from './mode';
import './ViewModeToggle.css';

/**
 * Chooses between the two arrangements of the same sessions.
 *
 * Two quiet buttons rather than a switch: a switch implies on and off, and
 * neither mode is the absence of the other. `aria-pressed` carries the state,
 * so the accent border is reinforcement rather than the only signal.
 *
 * Pressing the mode already showing is deliberately not a toggle. Someone who
 * cannot tell which mode they are in reaches for the label of the one they
 * want, and flipping them away from it would be the opposite of what they asked
 * for.
 */

interface ViewModeToggleProps {
  readonly mode: ViewMode;
  readonly onChange: (mode: ViewMode) => void;
}

const MODES: readonly { readonly value: ViewMode; readonly label: string }[] = [
  { value: 'tabs', label: 'Tabs' },
  { value: 'canvas', label: 'Canvas' },
];

export function ViewModeToggle({ mode, onChange }: ViewModeToggleProps) {
  return (
    <div className="viewmode" role="group" aria-label="View mode">
      {MODES.map((option) => (
        <button
          key={option.value}
          type="button"
          className="viewmode__option"
          aria-pressed={mode === option.value}
          data-selected={mode === option.value}
          onClick={() => {
            onChange(option.value);
          }}
        >
          {option.label}
        </button>
      ))}
    </div>
  );
}
