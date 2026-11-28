import { useCallback, useMemo } from 'react';
import type { AppRoute } from '../routing/routes';
import { setAppearance } from '../theme/appearance';
import { CommandMenu, useCommandShortcut } from './CommandMenu';
import { shellCommands } from './commands';
import type { NavigationState } from './navigation';

/**
 * The console's command menu: its ⌘K shortcut and its entries, built from
 * the navigation the console has already read.
 */
export function ShellCommandMenu({
  open,
  onOpenChange,
  enabled,
  navigation,
  onNavigate,
  onOpenEnvironments,
  onNewAgent,
  canOpenTriggers,
  canOpenOperations,
}: {
  readonly open: boolean;
  readonly onOpenChange: (open: boolean) => void;
  /** Visitors get no command menu and no shortcut. */
  readonly enabled: boolean;
  readonly navigation: NavigationState;
  readonly onNavigate: (route: AppRoute) => void;
  readonly onOpenEnvironments: () => void;
  readonly onNewAgent?: () => void;
  readonly canOpenTriggers: boolean;
  readonly canOpenOperations: boolean;
}) {
  const toggle = useCallback(() => onOpenChange(!open), [onOpenChange, open]);
  useCommandShortcut(toggle, enabled);
  const commands = useMemo(
    () =>
      shellCommands({
        items: navigation.status === 'ready' ? navigation.items : [],
        onNavigate,
        onOpenEnvironments,
        onNewAgent,
        canOpenTriggers,
        canOpenOperations,
        onAppearance: setAppearance,
      }),
    [navigation, onNavigate, onOpenEnvironments, onNewAgent, canOpenTriggers, canOpenOperations],
  );
  if (!enabled) return null;
  return <CommandMenu open={open} onClose={() => onOpenChange(false)} commands={commands} />;
}
