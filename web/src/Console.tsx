import { useState } from 'react';
import type { User } from './auth/types';
import { ProfileMenu } from './profile/ProfileMenu';
import type { AppRoute } from './routing/routes';
import { AppFrame } from './shell/AppFrame';
import { Sidebar } from './shell/Sidebar';
import './Console.css';

interface ConsoleProps {
  readonly user: User;
  readonly route: AppRoute;
  readonly onSignOut: () => void;
}

export function Console({ user, route, onSignOut }: ConsoleProps) {
  const [sidebarCollapsed, setSidebarCollapsed] = useState(false);
  const content =
    route.kind === 'not_found' ? (
      <section className="console__empty" aria-labelledby="not-found-title">
        <p className="console__eyebrow">404</p>
        <h1 id="not-found-title">Page not found</h1>
        <a href="/">Return to CMA</a>
      </section>
    ) : (
      <section className="console__empty" aria-labelledby="empty-title">
        <p className="console__eyebrow">CMA</p>
        <h1 id="empty-title">No resources yet</h1>
      </section>
    );

  return (
    <AppFrame
      detailsOpen={false}
      sidebarCollapsed={sidebarCollapsed}
      onSidebarCollapsedChange={setSidebarCollapsed}
      sidebar={({ collapsed, onToggle }) => (
        <Sidebar
          collapsed={collapsed}
          onToggleCollapsed={onToggle}
          account={<ProfileMenu user={user} collapsed={collapsed} onSignOut={onSignOut} />}
        />
      )}
      center={<main className="console__surface">{content}</main>}
    />
  );
}
