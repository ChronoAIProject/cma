import { cases as account } from './account.mjs';
import { cases as navigation } from './navigation.mjs';
import { cases as creation } from './creation.mjs';
import { cases as turns } from './turns.mjs';
import { cases as queue } from './queue.mjs';
import { cases as approvals } from './approvals.mjs';
import { cases as composer } from './composer.mjs';

export const cases = [
  ...account,
  ...navigation,
  ...creation,
  ...turns,
  ...queue,
  ...approvals,
  ...composer,
];
