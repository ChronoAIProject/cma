import { useState } from 'react';
import type { AgentSettingsView } from '../api/client';
import { Menu } from '../ui/Menu';
import { IconShieldOutline16 } from '../ui/glyphs';
import { PERMISSION_CHOICES, permissionLabel } from './settingsChoices';

type Permission = AgentSettingsView['permission_mode'];

/** The composer's permission mode, as a quiet chip opening the shared dropdown (§13). */
export function PermissionChip({
  value,
  disabled,
  onChange,
}: {
  readonly value: Permission;
  readonly disabled: boolean;
  readonly onChange: (value: Permission) => void;
}) {
  const [open, setOpen] = useState(false);
  const label = permissionLabel(value);
  return (
    <Menu
      open={open && !disabled}
      placement="agent-settings__permission"
      side="top"
      align="start"
      dense
      label="Permission mode"
      selectedId={value}
      items={PERMISSION_CHOICES.map((choice) => ({ id: choice.value, label: choice.label }))}
      onClose={() => setOpen(false)}
      onSelect={(id) => {
        const chosen = PERMISSION_CHOICES.find((choice) => choice.value === id);
        if (chosen !== undefined) onChange(chosen.value);
        setOpen(false);
      }}
      anchor={
        <button
          type="button"
          className="composer-chip"
          aria-label="Permission mode"
          aria-haspopup="menu"
          aria-expanded={open && !disabled}
          title={`Permission mode: ${label}`}
          disabled={disabled}
          onClick={() => setOpen((was) => !was)}
        >
          <IconShieldOutline16 />
          <span className="composer-chip__label">{label}</span>
        </button>
      }
    />
  );
}
