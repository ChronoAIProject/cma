import { useState } from 'react';
import { Menu } from '../ui/Menu';
import { IconChevronDownOutline14 } from '../ui/icons';

export type SteeringMode = 'interrupt' | 'queue';

export function SteeringModePicker({
  value,
  onChange,
  disabled = false,
}: {
  readonly value: SteeringMode;
  readonly onChange: (value: SteeringMode) => void;
  readonly disabled?: boolean;
}) {
  const [open, setOpen] = useState(false);
  const label = value === 'queue' ? 'Queue' : 'Interrupt';
  return (
    <Menu
      open={open && !disabled}
      placement="agent-composer__mode"
      side="top"
      align="end"
      dense
      label="Steering mode"
      selectedId={value}
      items={[
        { id: 'interrupt', label: 'Interrupt', reason: 'Send new input into the active turn.' },
        { id: 'queue', label: 'Queue', reason: 'Wait until the current turn finishes.' },
      ]}
      onClose={() => setOpen(false)}
      onSelect={(mode) => {
        if (mode === 'queue' || mode === 'interrupt') onChange(mode);
        setOpen(false);
      }}
      anchor={
        <button
          type="button"
          className="agent-composer__mode-button"
          aria-label={`Steering mode: ${label}`}
          aria-haspopup="menu"
          aria-expanded={open && !disabled}
          title={
            value === 'queue'
              ? 'Queue new messages until the turn finishes'
              : 'Steer the active turn with new input'
          }
          disabled={disabled}
          onClick={() => setOpen((current) => !current)}
        >
          {label}
          <IconChevronDownOutline14 />
        </button>
      }
    />
  );
}
