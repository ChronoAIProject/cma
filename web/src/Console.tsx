import type { ConsoleProps } from './ConsoleTypes';
import { CmaReadScope, useCmaClient } from './api/context';
import { useIdentityUncertain } from './auth/context';
import { OwnerViewsProvider } from './resources/retainedOwnerViews';
import { useDeletionNoticeReconciliation } from './useDeletionNoticeReconciliation';
import { ConsoleContent } from './ConsoleContent';

export function Console(props: ConsoleProps) {
  const { client, settlements } = useDeletionNoticeReconciliation(
    useCmaClient(),
    props.user.id,
    props.authEpoch ?? 0,
    !useIdentityUncertain(),
  );
  return (
    <CmaReadScope client={client}>
      <OwnerViewsProvider key={props.user.id} viewerId={props.user.id}>
        <ConsoleContent {...props} settlements={settlements} />
      </OwnerViewsProvider>
    </CmaReadScope>
  );
}
