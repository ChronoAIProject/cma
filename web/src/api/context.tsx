import { createContext, useContext } from 'react';
import type { ReactNode } from 'react';
import { unavailableClient, type CmaWebClient } from './client';

const CmaClientContext = createContext<CmaWebClient>(unavailableClient);

interface CmaClientProviderProps {
  readonly client: CmaWebClient;
  readonly children: ReactNode;
}

export function CmaClientProvider({ client, children }: CmaClientProviderProps) {
  return <CmaClientContext value={client}>{children}</CmaClientContext>;
}

export function useCmaClient(): CmaWebClient {
  return useContext(CmaClientContext);
}
