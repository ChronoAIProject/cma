import { cases as computer } from './computer.mjs';
import { cases as sandboxes } from './sandboxes.mjs';
import { cases as workspaces } from './workspaces.mjs';
import { cases as connections } from './connections.mjs';
import { cases as sharing } from './sharing.mjs';
import { cases as artifacts } from './artifacts.mjs';
import { cases as documents } from './documents.mjs';
import { cases as triggers } from './triggers.mjs';
export const cases = [
  ...computer,
  ...sandboxes,
  ...workspaces,
  ...connections,
  ...sharing,
  ...artifacts,
  ...documents,
  ...triggers,
];
