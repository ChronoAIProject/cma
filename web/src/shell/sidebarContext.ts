import { createContext } from 'react';
export const SidebarContext = createContext<{ collapsed: boolean; toggle: () => void } | undefined>(
  undefined,
);

export const NavigationSettingsContext = createContext<HTMLElement | null>(null);
