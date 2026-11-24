import { canonicalJson, compare, normalizeRecords, sha256 } from './model.mjs';

const cell = (value) => String(value).replaceAll('&', '&amp;').replaceAll('<', '&lt;')
  .replaceAll('>', '&gt;').replaceAll('|', '&#124;').replaceAll('\n', ' ');

export function makeInventory(records, inputHashes, tools, unmappedNotices) {
  const counts = {};
  for (const record of records) counts[record.ecosystem] = (counts[record.ecosystem] ?? 0) + 1;
  return {
    schemaVersion: 1,
    status: 'DRAFT — third-party distribution review remains',
    scope: 'Cargo all-features/all-targets metadata, every npm lock location, and declared vendored source/assets; not a release SBOM',
    counts,
    tools,
    // Keep filenames separate from digest values: an API-named file is not an API key.
    inputHashes: Object.entries(inputHashes).sort(([a], [b]) => compare(a, b))
      .map(([path, digest]) => ({ path, sha256: digest })),
    inputDigest: sha256(canonicalJson(inputHashes)),
    unmappedNotices,
    records: normalizeRecords(records),
  };
}

function table(records) {
  return [
    '| Ecosystem | Package | Version | Declared license | Evidence | Review / scope flags |',
    '| --- | --- | --- | --- | --- | --- |',
    ...records.map((record) => `| ${[record.ecosystem, record.name, record.version, record.license,
      record.evidence, record.flags.join(', ') || 'metadata only'].map(cell).join(' | ')} |`),
    '',
  ];
}

export function renderNotices(inventory) {
  const thirdParty = inventory.records.filter((record) => record.ecosystem !== 'workspace');
  return [
    '# THIRD_PARTY_NOTICES — DRAFT', '',
    'The first-party source scope is licensed under Apache-2.0 and the first OSS release is source-only.',
    'This inventory does not approve third-party redistribution or substitute for artifact-specific review.',
    'Third-party materials retain their own terms. License alternatives have not been selected.', '',
    `Input SHA-256: ${inventory.inputDigest}`, '',
    '## Scope and limitations', '',
    inventory.scope, '',
    'All development, build, optional and platform-specific npm lock locations are included.',
    'Rows count occurrences, not unique components or packages installed on one machine.',
    'Cargo workspace members are reported separately and never treated as third-party grants.',
    'Raw registry URLs and machine paths are omitted; source digests distinguish origins.', '',
    'Package metadata is evidence, not the full license. Before distribution, retrieve the exact',
    'copyright, license and NOTICE texts for each selected component and preserve required notices.',
    'Bundled transitive sources, native libraries, media, generated code and artifacts need separate',
    'inspection. The vendor catalog is an explicit starting inventory, not an exhaustive code scan.',
    'Web bundles, binaries and images require artifact-specific inventories and source-offer review.', '',
    '## Inventory', '', ...table(thirdParty),
    '## Project workspace declarations', '',
    'The project has adopted Apache-2.0. NOASSERTION on a workspace row means its Cargo',
    'manifest has no license declaration; it does not reopen the project license decision.', '',
    ...table(inventory.records.filter((record) => record.ecosystem === 'workspace')),
    '## Unmapped notice files', '',
    ...(inventory.unmappedNotices.length ? inventory.unmappedNotices.map((path) => `- ${cell(path)} — review and extend the vendor catalog.`) : ['No additional notice filenames found; this does not prove all copied code or media is cataloged.']), '',
    'See COPYLEFT_REVIEW.md for potential obligations and inventory.json for exact input hashes,',
    'origin digests, vendored file lists and unresolved notice paths.', '',
  ].join('\n');
}

export function renderReview(inventory, policy) {
  const lines = ['# License obligation review — DRAFT', '',
    'These are potential obligations for review, not conclusions about the whole project.',
    'An OR alternative remains an open choice; AND and WITH terms remain intact in each row.',
    'Review the complete expression, exact upstream texts, modifications and distribution form.',
    'No gate pass is implied, including for recognized permissive licenses.', '',
    `Input SHA-256: ${inventory.inputDigest}`, ''];
  const used = [...new Set(inventory.records.flatMap((record) => record.obligations))].sort(compare);
  for (const id of used) {
    const obligation = policy.obligations[id];
    lines.push(`## ${id} (${obligation.category})`, '', obligation.summary, '',
      `Authority: ${obligation.source}`, '',
      ...table(inventory.records.filter((record) => record.obligations.includes(id))));
  }
  lines.push('## Unresolved declarations and notices', '', ...table(inventory.records.filter((record) =>
    record.flags.some((flag) => /missing|review|pending/.test(flag)) && record.obligations.length === 0)),
  'Known categories are triage rules only. Any unrecognized or custom declaration requires review.',
  'No obligations may be dropped merely because a package is dev-only, optional or absent on the current platform.', '');
  return lines.join('\n');
}
