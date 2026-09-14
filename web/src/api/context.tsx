import { createContext, useContext } from 'react';
import type { ReactNode } from 'react';
import { unavailableClient, type CmaWebClient } from './client';
import type { ProfileViewsClient } from '../profiles/ProfileViews';
import type { CreationClient } from '../create/client';
import type { OperatorClient } from '../operator/client';
import type { TriggerPublisherClient } from '../trigger-publisher/client';
import type { PublicClient } from '../public/client';

const CmaClientContext = createContext<CmaWebClient>(unavailableClient);
const ProfileClientContext = createContext<ProfileViewsClient | undefined>(undefined);
const CreationClientContext = createContext<CreationClient | undefined>(undefined);
const OperatorClientContext = createContext<OperatorClient | undefined>(undefined);
const PublisherClientContext = createContext<TriggerPublisherClient | undefined>(undefined);
const PublicClientContext = createContext<PublicClient | undefined>(undefined);

interface CmaClientProviderProps {
  readonly client: CmaWebClient;
  readonly profiles?: ProfileViewsClient;
  readonly creation?: CreationClient;
  readonly operator?: OperatorClient;
  readonly publisher?: TriggerPublisherClient;
  readonly publicClient?: PublicClient;
  readonly children: ReactNode;
}

export function CmaClientProvider({
  client,
  profiles,
  creation,
  operator,
  publisher,
  publicClient,
  children,
}: CmaClientProviderProps) {
  return (
    <CmaClientContext value={client}>
      <ProfileClientContext value={profiles}>
        <CreationClientContext value={creation}>
          <OperatorClientContext value={operator}>
            <PublisherClientContext value={publisher}>
              <PublicClientContext value={publicClient}>{children}</PublicClientContext>
            </PublisherClientContext>
          </OperatorClientContext>
        </CreationClientContext>
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

export function useOperatorClient(): OperatorClient | undefined {
  return useContext(OperatorClientContext);
}

export function usePublisherClient(): TriggerPublisherClient | undefined {
  return useContext(PublisherClientContext);
}

export function usePublicClient(): PublicClient | undefined {
  return useContext(PublicClientContext);
}
