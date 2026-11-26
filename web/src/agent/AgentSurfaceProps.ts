import type { CmaWebClient } from '../api/client';
import type { AppRoute } from '../routing/routes';
import type { TreeAction, TreePermissions } from '../shell/Tree';
import type { SelectedRetiredAgent } from '../useDeletionNoticeReconciliation';
import type { AgentController } from './controller';
import type { AgentDraft } from './draft';
import type { AgentComputerObservation } from './environment/observation';
import type { MessageUser } from './MessageIdentity';
import type { SuggestedReplySet } from './suggestions';
import type { AgentViewPosition } from './viewPosition';

export interface AgentSurfaceProps {
  readonly onPersonalHome?: () => void;
  readonly selectedRetired?: SelectedRetiredAgent;
  readonly computerObservation?: AgentComputerObservation;
  readonly onViewDeletionStatus?: () => void;
  readonly suggestedReplies?: SuggestedReplySet | null;
  readonly lifecycleClient?: CmaWebClient;
  readonly accessUncertain?: boolean;
  /** Current personal access was checked for this exact Agent before mounting. */
  readonly accessAuthorized?: boolean;
  readonly firstSendContext?: { readonly authEpoch: number; readonly visible: boolean };
  readonly draft?: AgentDraft;
  readonly viewerId?: string;
  readonly user?: MessageUser;
  readonly agentId: string;
  readonly controller: AgentController;
  readonly viewPosition?: AgentViewPosition;
  readonly filesOpen?: boolean;
  readonly onFilesOpenChange?: (open: boolean) => void;
  readonly detailsOpen: boolean;
  readonly onDetailsOpenChange: (open: boolean) => void;
  readonly onNavigate: (route: AppRoute) => void;
  readonly permissions?: TreePermissions;
  /** Another resource action is still being confirmed. */
  readonly actionsBusy?: boolean;
  readonly onAgentAction?: (action: TreeAction) => void;
}
