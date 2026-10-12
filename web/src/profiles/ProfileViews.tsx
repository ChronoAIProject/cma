import type { EventWorkerProfiles } from '@chronoai/cma-sdk';
import type { CreationClient } from '../create/client';
import { ProfileLibrary } from './ProfileLibrary';
import { lazy, Suspense } from 'react';
const EventWorkersHome = lazy(() =>
  import('./event-workers/Home').then((module) => ({ default: module.EventWorkersHome })),
);
const EventWorkerEditor = lazy(() =>
  import('./event-workers/Editor').then((module) => ({ default: module.EventWorkerEditor })),
);
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
  {
    kind:
      | 'profiles'
      | 'profile'
      | 'marketplace'
      | 'marketplace_profile'
      | 'event_worker_profiles'
      | 'event_worker_profile';
  }
>;

export interface ProfileViewsClient {
  readonly eventWorkers?: EventWorkerProfiles;
  readonly creation?: CreationClient;
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
  viewerId,
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
  readonly viewerId?: string;
}) {
  switch (route.kind) {
    case 'profiles':
      return (
        <ProfileLibrary active="profiles" onNavigate={onNavigate}>
          <ProfilesHome
            client={client.profiles}
            onNavigate={onNavigate}
            canManage={canManage}
            onNewProfile={(profileKind) =>
              onNavigate({ kind: 'profile', profileKind, profileId: 'new' })
            }
          />
        </ProfileLibrary>
      );
    case 'marketplace':
      return (
        <ProfileLibrary active="marketplace" onNavigate={onNavigate}>
          <Marketplace client={client.marketplace} onNavigate={onNavigate} />
        </ProfileLibrary>
      );
    case 'event_worker_profiles':
      return (
        <ProfileLibrary active="event_worker_profiles" onNavigate={onNavigate}>
          {client.eventWorkers === undefined ? (
            <p>Event Worker Profiles are unavailable.</p>
          ) : (
            <Suspense fallback={<p role="status">Loading Event Worker Profiles</p>}>
              <EventWorkersHome
                client={client.eventWorkers}
                onNavigate={onNavigate}
                canManage={canManage}
              />
            </Suspense>
          )}
        </ProfileLibrary>
      );
    case 'event_worker_profile':
      return canManage &&
        viewerId !== undefined &&
        client.eventWorkers !== undefined &&
        client.creation !== undefined ? (
        <Suspense fallback={<p role="status">Loading Event Worker Profile</p>}>
          <EventWorkerEditor
            key={`${viewerId}:${route.profileId}`}
            viewerId={viewerId}
            client={client.eventWorkers}
            profiles={client.creation}
            profileId={route.profileId}
            onNavigate={onNavigate}
          />
        </Suspense>
      ) : (
        <section className="profile-editor">
          <h1>Profile unavailable</h1>
          <p>You do not have permission to manage Event Worker Profiles.</p>
        </section>
      );
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
