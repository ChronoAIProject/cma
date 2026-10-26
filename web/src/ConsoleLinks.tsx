import type { AppRoute } from './routing/routes';
import type { SidebarLink } from './shell/Sidebar';
import { IconGoalOutline16, IconFolderOpenOutline16 } from './ui/icons';
export function consoleLinks(
  route: AppRoute,
  triggers: boolean,
  documents: boolean,
): SidebarLink[] {
  return [
    ...(documents
      ? [
          {
            id: 'artifacts',
            label: 'Artifacts',
            icon: <IconFolderOpenOutline16 />,
            route: { kind: 'artifacts' } as const,
            current: route.kind === 'artifacts' || route.kind === 'document',
          },
        ]
      : []),
    ...(triggers
      ? [
          {
            id: 'triggers',
            label: 'Triggers',
            icon: <IconGoalOutline16 />,
            route: { kind: 'triggers' } as const,
            current:
              route.kind === 'triggers' ||
              route.kind === 'trigger' ||
              route.kind === 'trigger_embed',
          },
        ]
      : []),
  ];
}
