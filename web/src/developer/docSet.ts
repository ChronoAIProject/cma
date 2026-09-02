import type { PhrasingContent, RootContent } from 'mdast';
import { parse } from '../chat/markdown';

export const SDK_LANGUAGES = ['typescript', 'rust'] as const;
export type SdkLanguage = (typeof SDK_LANGUAGES)[number];

const SHARED_SDK_DOCUMENT_FILENAMES = [
  'README.md',
  'quickstart.md',
  'authentication.md',
  'sessions.md',
  'streaming.md',
  'triggers.md',
  'operations.md',
  'errors.md',
  'reference.md',
] as const;

export const SDK_DOCUMENT_FILENAMES = {
  typescript: [...SHARED_SDK_DOCUMENT_FILENAMES, 'sponsorships.md'],
  rust: SHARED_SDK_DOCUMENT_FILENAMES,
} as const satisfies Readonly<Record<SdkLanguage, readonly string[]>>;

export type SdkDocumentFilename = (typeof SDK_DOCUMENT_FILENAMES)[SdkLanguage][number];

export interface SdkDocument {
  readonly sdk: SdkLanguage;
  readonly filename: SdkDocumentFilename;
  readonly title: string;
  readonly label: string;
  readonly order: number;
  /** Repository markdown with only the nav metadata comment removed. */
  readonly markdown: string;
}

export type SdkDocumentSets = Readonly<Record<SdkLanguage, readonly SdkDocument[]>>;
export type RawSdkDocuments = Readonly<Record<string, unknown>>;

const SOURCE_PATH = /(?:^|\/)sdk\/(typescript|rust)\/docs\/([^/]+\.md)$/;
const NAV = /^<!-- nav: ([1-9]\d*) ([^<>\r\n]+?) -->$/;

function fail(message: string): never {
  throw new Error(`SDK documentation: ${message}`);
}

function words(nodes: readonly PhrasingContent[]): string {
  return nodes
    .map((node) => {
      if ('value' in node && typeof node.value === 'string') {
        return node.value;
      }
      if ('children' in node) {
        return words(node.children as readonly PhrasingContent[]);
      }
      return node.type === 'image' ? (node.alt ?? '') : '';
    })
    .join('')
    .trim();
}

function compileDocument(
  sdk: SdkLanguage,
  filename: SdkDocumentFilename,
  source: string,
): SdkDocument {
  let tree;
  try {
    tree = parse(source);
  } catch (cause) {
    return fail(`${sdk}/${filename} cannot be parsed: ${String(cause)}`);
  }

  const headings = tree.children.filter(
    (node): node is Extract<RootContent, { type: 'heading' }> =>
      node.type === 'heading' && node.depth === 1,
  );
  const first = tree.children[0];
  if (headings.length !== 1 || first?.type !== 'heading' || first.depth !== 1) {
    return fail(`${sdk}/${filename} must begin with its only H1`);
  }
  const title = words(first.children);
  if (title === '') {
    return fail(`${sdk}/${filename} has an empty H1`);
  }

  const navNodes = tree.children.filter(
    (node): node is Extract<RootContent, { type: 'html' }> =>
      node.type === 'html' && node.value.trim().startsWith('<!-- nav'),
  );
  const navNode = navNodes[0];
  if (navNodes.length !== 1 || navNode === undefined || tree.children[1] !== navNode) {
    return fail(`${sdk}/${filename} must put exactly one nav comment after its H1`);
  }
  const match = NAV.exec(navNode.value.trim());
  if (match === null) {
    return fail(`${sdk}/${filename} has malformed nav metadata`);
  }
  const order = Number(match[1]);
  const label = match[2]?.trim() ?? '';
  if (!Number.isSafeInteger(order) || label === '') {
    return fail(`${sdk}/${filename} has malformed nav metadata`);
  }

  const start = navNode.position?.start.offset;
  const end = navNode.position?.end.offset;
  if (start === undefined || end === undefined) {
    return fail(`${sdk}/${filename} nav metadata has no source position`);
  }

  return {
    sdk,
    filename,
    title,
    label,
    order,
    markdown: `${source.slice(0, start)}${source.slice(end)}`,
  };
}

/** Compile and validate the complete pair of SDK guide sets. */
export function compileSdkDocuments(sources: RawSdkDocuments): SdkDocumentSets {
  const bySdk: Record<SdkLanguage, Map<string, string>> = {
    typescript: new Map(),
    rust: new Map(),
  };

  for (const [path, raw] of Object.entries(sources).sort(([left], [right]) =>
    left.localeCompare(right),
  )) {
    const match = SOURCE_PATH.exec(path);
    if (match === null) {
      return fail(`unexpected source path ${path}`);
    }
    const sdk = match[1] as SdkLanguage;
    const filename = match[2] ?? '';
    if (typeof raw !== 'string') {
      return fail(`${sdk}/${filename} was not imported as raw text`);
    }
    if (bySdk[sdk].has(filename)) {
      return fail(`${sdk}/${filename} was imported more than once`);
    }
    bySdk[sdk].set(filename, raw);
  }

  const output = {} as Record<SdkLanguage, readonly SdkDocument[]>;
  for (const sdk of SDK_LANGUAGES) {
    const found = bySdk[sdk];
    const filenames: readonly SdkDocumentFilename[] = SDK_DOCUMENT_FILENAMES[sdk];
    const expected = new Set<string>(filenames);
    const missing = filenames.filter((filename) => !found.has(filename));
    const extra = [...found.keys()].filter((filename) => !expected.has(filename));
    if (missing.length > 0 || extra.length > 0) {
      const details = [
        missing.length === 0 ? '' : `missing ${missing.join(', ')}`,
        extra.length === 0 ? '' : `unexpected ${extra.join(', ')}`,
      ]
        .filter((part) => part !== '')
        .join('; ');
      return fail(`${sdk} guide set is incomplete: ${details}`);
    }

    const documents = filenames
      .map((filename) => compileDocument(sdk, filename, found.get(filename) ?? ''))
      .sort((left, right) => left.order - right.order);
    const orders = new Set<number>();
    const labels = new Set<string>();
    for (const document of documents) {
      const normalLabel = document.label.toLocaleLowerCase();
      if (orders.has(document.order)) {
        return fail(`${sdk} has duplicate nav order ${String(document.order)}`);
      }
      if (labels.has(normalLabel)) {
        return fail(`${sdk} has duplicate nav label ${document.label}`);
      }
      orders.add(document.order);
      labels.add(normalLabel);
    }
    output[sdk] = documents;
  }

  return output;
}
