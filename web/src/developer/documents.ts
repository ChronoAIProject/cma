import { compileSdkDocuments } from './docSet';

/** Repository guides become immutable build inputs; there is no docs request. */
const rawDocuments = import.meta.glob('../../../sdk/*/docs/*.md', {
  eager: true,
  import: 'default',
  query: '?raw',
}) as Record<string, string>;

export const SDK_DOCUMENTS = compileSdkDocuments(rawDocuments);
