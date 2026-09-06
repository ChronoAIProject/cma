import type { AppRoute } from '../routing/routes';
import type {
  CapacityBounds,
  ProfileAuthoringClient,
  ProfileContents,
  ProfileEditorClient,
  ProfilesClient,
  RepositoryResolver,
  ServiceCatalogClient,
  SkillCatalogClient,
} from './client';
import { AgentProfileEditor } from './AgentProfileEditor';
import { WorkspaceProfileEditor } from './WorkspaceProfileEditor';
import { SandboxProfileEditor } from './SandboxProfileEditor';
import { ProfilesHome } from './ProfilesHome';
import type { MarketplaceClient, ProfileUse } from './marketplace/client';
import { Marketplace } from './marketplace/Marketplace';
import { ProfileDetail } from './marketplace/ProfileDetail';

export type ProfileRoute = Extract<
  AppRoute,
  { kind: 'profiles' | 'profile' | 'marketplace' | 'marketplace_profile' }
>;

export interface ProfileViewsClient {
  readonly profiles: ProfilesClient;
  readonly agent: ProfileEditorClient<ProfileContents['agent']>;
  readonly workspace: ProfileEditorClient<ProfileContents['workspace']>;
  readonly sandbox: ProfileEditorClient<ProfileContents['sandbox']>;
  readonly marketplace: MarketplaceClient;
  readonly authoring?: ProfileAuthoringClient;
}

export function ProfileViews({
  route,
  client,
  onNavigate,
  onUse,
  bounds,
  resolver,
  services,
  skills,
  canManage = true,
}: {
  readonly route: ProfileRoute;
  readonly client: ProfileViewsClient;
  readonly onNavigate: (route: AppRoute) => void;
  readonly onUse?: (selection: ProfileUse) => void;
  readonly bounds?: CapacityBounds;
  readonly resolver?: RepositoryResolver;
  readonly services?: ServiceCatalogClient;
  readonly skills?: SkillCatalogClient;
  readonly canManage?: boolean;
}) {
  switch (route.kind) {
    case 'profiles':
      return (
        <ProfilesHome
          client={client.profiles}
          onNavigate={onNavigate}
          canManage={canManage}
          onNewProfile={(profileKind) =>
            onNavigate({ kind: 'profile', profileKind, profileId: 'new' })
          }
        />
      );
    case 'marketplace':
      return <Marketplace client={client.marketplace} onNavigate={onNavigate} />;
    case 'marketplace_profile':
      return (
        <ProfileDetail
          client={client.marketplace}
          kind={route.profileKind}
          profileId={route.profileId}
          onNavigate={onNavigate}
          onUse={onUse}
          canManage={canManage}
        />
      );
    case 'profile': {
      if (!canManage)
        return (
          <section className="profile-editor">
            <h1>Profile unavailable</h1>
            <p>You do not have permission to manage profiles.</p>
          </section>
        );
      const profileId = route.profileId === 'new' ? undefined : route.profileId;
      const onSaved = ({ id }: { readonly id: string }) => {
        if (profileId === undefined)
          onNavigate({ kind: 'profile', profileKind: route.profileKind, profileId: id });
      };
      switch (route.profileKind) {
        case 'agent':
          return (
            <AgentProfileEditor
              client={client.agent}
              catalog={skills ?? client.authoring}
              profileId={profileId}
              onSaved={onSaved}
            />
          );
        case 'workspace':
          return (
            <WorkspaceProfileEditor
              client={client.workspace}
              profiles={client.profiles}
              marketplace={client.marketplace}
              resolver={resolver ?? client.authoring}
              profileId={profileId}
              onSaved={onSaved}
            />
          );
        case 'sandbox':
          return (
            <SandboxProfileEditor
              client={client.sandbox}
              profiles={client.profiles}
              marketplace={client.marketplace}
              services={services ?? client.authoring}
              bounds={bounds}
              boundsClient={client.authoring}
              profileId={profileId}
              onSaved={onSaved}
            />
          );
      }
    }
  }
}
