import { createContext } from 'react';
import type { AnswerDeliverable } from '@chronoai/cma-sdk';
import type { AnswerDeliveryAccess } from './answerDeliveries';

export interface DeliverableSelection {
  readonly access: AnswerDeliveryAccess;
  readonly answerId: string;
  readonly item: AnswerDeliverable;
  readonly trigger: HTMLElement;
}

export const DeliverablePreviewContext = createContext<
  | {
      readonly selected?: DeliverableSelection;
      readonly open: (selection: DeliverableSelection) => void;
      readonly invalidate: (access: AnswerDeliveryAccess) => void;
    }
  | undefined
>(undefined);
