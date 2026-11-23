import type { PropsWithChildren } from 'react';
import { TaskHomeDecorationContext } from '../create/TaskHomeDecoration';
import { TopographicBackdrop } from './TopographicBackdrop';

const decoration = <TopographicBackdrop placement="first-visit" />;

export function HomeDecorationProvider({ children }: PropsWithChildren) {
  return (
    <TaskHomeDecorationContext.Provider value={decoration}>
      {children}
    </TaskHomeDecorationContext.Provider>
  );
}
