import type { AppRoute } from '../routing/routes';
import type { CreationClient } from '../create/client';
import type { BotClient } from './model';
import type { AgentProfileContent, ProfileEditorClient } from '../profiles/client';
import { BotNew } from './BotNew';
import { useCreationIntent } from './creationIntent';
import { BotDetail } from './BotDetail';
import { BotsList } from './BotsList';
import './Bots.css';

export type BotRoute = Extract<AppRoute, { kind: 'bots' | 'bot_new' | 'bot' }>;
export function BotPage({
  client,
  viewer,
  viewerLabel = viewer,
  profiles,
  agentProfiles,
  route,
  onNavigate,
}: {
  readonly client: BotClient;
  readonly viewer: string;
  readonly viewerLabel?: string;
  readonly profiles?: CreationClient;
  readonly agentProfiles?: ProfileEditorClient<AgentProfileContent>;
  readonly route: BotRoute;
  readonly onNavigate: (route: AppRoute) => void;
}) {
  const { intent, retain, error } = useCreationIntent(viewer);
  return (
    <div className="bots">
      {route.kind === 'bots' ? (
        <BotsList client={client} onNavigate={onNavigate} />
      ) : route.kind === 'bot' ? (
        <BotDetail key={route.botId} client={client} id={route.botId} onNavigate={onNavigate} />
      ) : error !== undefined ? (
        <section>
          <h1>Bot creation unavailable</h1>
          <p role="alert">{error}</p>
        </section>
      ) : profiles === undefined ? (
        <section>
          <h1>Profile selection unavailable</h1>
        </section>
      ) : (
        <BotNew
          client={client}
          viewer={viewer}
          viewerLabel={viewerLabel}
          profiles={profiles}
          agentProfiles={agentProfiles}
          onNavigate={onNavigate}
          intent={intent}
          retain={retain}
        />
      )}
    </div>
  );
}
