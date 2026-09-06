import { createContext, useContext } from 'react';
import type { ReactNode } from 'react';
import { unavailableClient, type CmaWebClient } from './client';
import type { ProfileViewsClient } from '../profiles/ProfileViews';

const CmaClientContext = createContext<CmaWebClient>(unavailableClient);
const ProfileClientContext = createContext<ProfileViewsClient | undefined>(undefined);

interface CmaClientProviderProps {
  readonly client: CmaWebClient;
  readonly profiles?: ProfileViewsClient;
  readonly children: ReactNode;
}

export function CmaClientProvider({ client, profiles, children }: CmaClientProviderProps) {
  return (
    <CmaClientContext value={client}>
      <ProfileClientContext value={profiles}>{children}</ProfileClientContext>
    </CmaClientContext>
  );
}

export function useProfileViewsClient(): ProfileViewsClient | undefined {
  return useContext(ProfileClientContext);
}

export function useCmaClient(): CmaWebClient {
  return useContext(CmaClientContext);
}
