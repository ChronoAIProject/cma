import type { TriggerAuthError } from '../routing/routes';
import { IconWarningOutline16 } from '../ui/icons';

const COPY: Readonly<
  Record<TriggerAuthError, { readonly title: string; readonly detail: string }>
> = {
  expired: {
    title: 'Sign-in expired',
    detail: 'Your sign-in request is no longer valid. Sign in again to continue.',
  },
  cancelled: {
    title: 'Sign-in was cancelled',
    detail: 'Nothing was started. Sign in again when you are ready.',
  },
  failed: {
    title: 'Sign-in failed',
    detail: 'CMA could not confirm your sign-in. Sign in again to retry.',
  },
};

interface TriggerAuthNoticeProps {
  readonly error: TriggerAuthError;
  readonly placement: 'public' | 'workflow';
}

export function TriggerAuthNotice({ error, placement }: TriggerAuthNoticeProps) {
  const copy = COPY[error];
  const placementClass =
    placement === 'public' ? 'trigger-public__auth-notice' : 'trigger-workflow__notice';
  return (
    <div className={`notice notice--fail ${placementClass}`} role="alert">
      <IconWarningOutline16 />
      <div>
        <p className="notice__title">{copy.title}</p>
        <p className="notice__detail">{copy.detail}</p>
      </div>
    </div>
  );
}
