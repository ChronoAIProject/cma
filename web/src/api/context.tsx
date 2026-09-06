import { createContext, useContext } from 'react';
import type { ReactNode } from 'react';
import { unavailableClient, type CmaWebClient } from './client';
import type { ProfileViewsClient } from '../profiles/ProfileViews';
import type { CreationClient } from '../create/client';

const CmaClientContext = createContext<CmaWebClient>(unavailableClient);
const ProfileClientContext = createContext<ProfileViewsClient | undefined>(undefined);
const CreationClientContext = createContext<CreationClient | undefined>(undefined);

interface CmaClientProviderProps {
  readonly client: CmaWebClient;
  readonly profiles?: ProfileViewsClient;
  readonly creation?: CreationClient;
  readonly children: ReactNode;
}

export function CmaClientProvider({
  client,
  profiles,
  creation,
  children,
}: CmaClientProviderProps) {
  return (
    <CmaClientContext value={client}>
      <ProfileClientContext value={profiles}>
        <CreationClientContext value={creation}>{children}</CreationClientContext>
      </ProfileClientContext>
    </CmaClientContext>
  );
}

export function useProfileViewsClient(): ProfileViewsClient | undefined {
  return useContext(ProfileClientContext);
}

export function useCmaClient(): CmaWebClient {
  return useContext(CmaClientContext);
}

export function useCreationClient(): CreationClient | undefined {
  return useContext(CreationClientContext);
}
