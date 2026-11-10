import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { resolveSymbols, type ContractCase } from '../public-contract/context.js';

export const directory = new URL(
  '../../../../api/crates/layer1-public/tests/fixtures/retained-clients/',
  import.meta.url,
);
interface Manifest {
  releases: {
    name: string;
    source_commit: string;
    fixtures: { file: string; sha256: string; cases: number; selected_case_ids: string[] }[];
  }[];
}
const manifest = JSON.parse(readFileSync(new URL('manifest.json', directory), 'utf8')) as Manifest;

export function retainedCorpora() {
  return manifest.releases.map((release) => ({
    name: release.name,
    commit: release.source_commit,
    cases: release.fixtures.flatMap((fixture) => {
      assert(!fixture.file.includes('/'));
      const bytes = readFileSync(new URL(fixture.file, directory));
      assert.equal(createHash('sha256').update(bytes).digest('hex'), fixture.sha256);
      const corpus = JSON.parse(bytes.toString('utf8')) as {
        version: number;
        cases: ContractCase[];
      };
      assert.equal(corpus.version, 1);
      assert.equal(corpus.cases.length, fixture.cases);
      assert.deepEqual(
        corpus.cases.map(({ id }) => id),
        fixture.selected_case_ids,
      );
      const scopes = new Map<string, Map<string, unknown>>();
      for (const item of corpus.cases) {
        const scope = scopes.get(item.scenario) ?? new Map<string, unknown>();
        scopes.set(item.scenario, scope);
        for (const [name, pointer] of Object.entries(item.captures ?? {})) {
          let value = item.response.body;
          for (const part of pointer.slice(1).split('/'))
            value = (value as Record<string, unknown>)[
              part.replace(/~1/g, '/').replace(/~0/g, '~')
            ];
          assert(value !== undefined && value !== null && typeof value !== 'object');
          if (scope.has(name)) assert.deepEqual(scope.get(name), value);
          scope.set(name, value);
        }
      }
      return corpus.cases.map((item) => ({
        ...item,
        captures: item.captures ?? {},
        request: resolveSymbols(
          { ...item.request, headers: item.request.headers ?? {} },
          scopes.get(item.scenario)!,
        ) as ContractCase['request'],
        response: resolveSymbols(
          item.response,
          scopes.get(item.scenario)!,
        ) as ContractCase['response'],
      }));
    }),
  }));
}
