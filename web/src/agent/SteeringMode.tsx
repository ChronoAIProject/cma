import { useState } from 'react';
import { Menu } from '../ui/Menu';
import { IconChevronDownOutline14, IconQueueOutline14, IconSendOutline14 } from '../ui/icons';

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
      placement="agent-composer__steering"
      side="top"
      align="end"
      dense
      label="Steering mode"
      selectedId={value}
      items={[
        {
          id: 'interrupt',
          label: 'Interrupt',
          reason: 'Stop the current turn, then start a new turn with this message.',
        },
        { id: 'queue', label: 'Queue', reason: 'Process after the current turn finishes.' },
      ]}
      onClose={() => setOpen(false)}
      onSelect={(mode) => {
        if (mode === 'queue' || mode === 'interrupt') onChange(mode);
        setOpen(false);
      }}
      anchor={
        <button
          type="button"
          className="composer-chip"
          aria-label={`Steering mode: ${label}`}
          aria-haspopup="menu"
          aria-expanded={open && !disabled}
          title={
            value === 'queue'
              ? 'Process after the current turn finishes.'
              : 'Stop the current turn, then start a new turn with this message.'
          }
          disabled={disabled}
          onClick={() => setOpen((current) => !current)}
        >
          {value === 'queue' ? <IconQueueOutline14 /> : <IconSendOutline14 />}
          <span className="composer-chip__label">{label}</span>
          <IconChevronDownOutline14 className="composer-chip__caret" />
        </button>
      }
    />
  );
}
