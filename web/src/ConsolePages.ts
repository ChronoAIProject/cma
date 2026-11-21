import { lazy } from 'react';

export const ProfileViews = lazy(() =>
  import('./profiles/ProfileViews').then((module) => ({ default: module.ProfileViews })),
);
export const Operations = lazy(() =>
  import('./operator/Operations').then((module) => ({ default: module.Operations })),
);

export const TriggerPublisherPage = lazy(() =>
  import('./trigger-publisher/TriggerPublisherPage').then((module) => ({
    default: module.TriggerPublisherPage,
  })),
);

export const Documents = lazy(() => import('./documents/Documents'));
