import type { ConsoleProps } from './ConsoleTypes';
import { CmaReadScope, useCmaClient } from './api/context';
import { useIdentityUncertain } from './auth/context';
import { OwnerViewsProvider } from './resources/retainedOwnerViews';
import { useDeletionNoticeReconciliation } from './useDeletionNoticeReconciliation';
import { ConsoleContent } from './ConsoleContent';

export function Console(props: ConsoleProps) {
  const { client, settlements, selectedRetired } = useDeletionNoticeReconciliation(
    useCmaClient(),
    props.user.id,
    props.authEpoch ?? 0,
    !useIdentityUncertain(),
    props.route.kind === 'agent' ? props.route.agentId : undefined,
  );
  return (
    <CmaReadScope client={client}>
      <OwnerViewsProvider key={props.user.id} viewerId={props.user.id}>
        <ConsoleContent {...props} settlements={settlements} selectedRetired={selectedRetired} />
      </OwnerViewsProvider>
    </CmaReadScope>
  );
}
