import { useState } from 'react';
import type { CmaWebClient } from '../api/client';
import { Button } from '../ui/Button';
import { Menu } from '../ui/Menu';
import { IconChevronDownOutline14, IconCloudOutline16 } from '../ui/icons';
import { useDesktopOption } from './useDesktopOption';

export function RunTargetMenu({
  client,
  workspaceId,
  sandboxId,
  desktop,
  machineIncluded,
  machineIncludedAvailable,
  frozen,
  policyAllowed,
  onDesktopChange,
  onMachineChange,
  onAvailable,
}: {
  readonly client: CmaWebClient;
  readonly workspaceId?: string;
  readonly sandboxId?: string;
  readonly desktop: boolean;
  readonly machineIncluded: boolean;
  readonly machineIncludedAvailable: boolean;
  readonly frozen: boolean;
  readonly policyAllowed: boolean;
  readonly onDesktopChange: (value: boolean) => void;
  readonly onMachineChange: (value: boolean) => void;
  readonly onAvailable: (value: boolean) => void;
}) {
  const [open, setOpen] = useState(false);
  const browser = useDesktopOption({
    client,
    workspaceId,
    sandboxId,
    enabled: desktop,
    frozen,
    policyAllowed,
    onAvailable,
  });
  const suffix =
    desktop && machineIncluded
      ? 'Browser + desktop'
      : desktop
        ? 'Browser'
        : machineIncluded
          ? 'Shared desktop'
          : undefined;
  return (
    <Menu
      open={open}
      presentation="run-target"
      label="Run this task on"
      onClose={() => setOpen(false)}
      selectedIds={[
        'cloud',
        ...(desktop ? ['browser'] : []),
        ...(machineIncluded ? ['shared'] : []),
      ]}
      anchor={
        <Button
          placement="task-home__tool"
          icon={<IconCloudOutline16 />}
          aria-haspopup="menu"
          aria-expanded={open}
          onClick={() => setOpen(!open)}
        >
          Cloud computer{suffix ? ` · ${suffix}` : ''}
          <IconChevronDownOutline14 />
        </Button>
      }
      items={[
        { type: 'label', id: 'location', text: 'Run this task on' },
        {
          id: 'cloud',
          role: 'menuitemradio',
          label: 'Cloud computer',
          reason:
            workspaceId || sandboxId
              ? "This Sandbox's existing Linux machine"
              : 'Its own Linux machine in the cloud',
          icon: <IconCloudOutline16 />,
        },
        { type: 'separator', id: 'divider' },
        { type: 'label', id: 'options', text: 'Computer options' },
        browser,
        {
          id: 'shared',
          role: 'menuitemcheckbox',
          label: 'Shared desktop',
          disabled: frozen || (!machineIncluded && !machineIncludedAvailable),
          reason: frozen
            ? 'Shared desktop choice is saved with the submitted task.'
            : machineIncludedAvailable
              ? 'Adds a Linux desktop with the Chromium browser that you and your Agent can both see and control. Useful for work on websites.'
              : 'A shared desktop can only be added with permission to create a new Sandbox; an existing Sandbox keeps its original setup.',
        },
      ]}
      onSelect={(id) => {
        if (id === 'browser') onDesktopChange(!desktop);
        if (id === 'shared') onMachineChange(!machineIncluded);
        if (id === 'cloud') setOpen(false);
      }}
    />
  );
}
