import type { Dispatch, RefObject, SetStateAction } from 'react';
import type { CmaWebClient, CommandKeys } from '../api/client';
import type { AgentLiveEvent } from '../api/types';
import type { AgentCommand, AgentResourceState } from './controllerTypes';
import type { AgentV2Session } from './transport/client';
import type { SetActionError } from './actionError';

export interface AgentActionOptions {
  readonly agentId?: string;
  readonly client: CmaWebClient;
  readonly historyIncomplete: boolean;
  readonly commandsBlocked: boolean;
  readonly activeTurn: boolean;
  readonly inputAvailability: { readonly allowed: boolean };
  readonly actionAbortRef: RefObject<AbortController | undefined>;
  readonly actionKindRef: RefObject<AgentCommand | undefined>;
  readonly replayAbortRef: RefObject<AbortController | undefined>;
  readonly stateRef: RefObject<AgentResourceState>;
  readonly keys: CommandKeys;
  readonly v2?: AgentV2Session;
  readonly setPending: Dispatch<SetStateAction<AgentCommand | undefined>>;
  readonly setActionError: SetActionError;
  readonly setReplayRevision: Dispatch<SetStateAction<number>>;
  readonly setRequestRevision: Dispatch<SetStateAction<number>>;
  readonly applyLive: (event: AgentLiveEvent) => void;
  readonly reload: () => void;
  readonly refreshDetail: () => void;
  readonly errorMessage: (error: unknown) => string;
  readonly onIdentityChange: (error: unknown) => boolean;
  readonly options: {
    readonly viewerId?: string;
    readonly onHierarchyChange?: () => void;
    readonly onDeleted?: () => void;
  };
}
