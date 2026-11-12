import { appendFileSync } from 'node:fs';
import { parseArgs } from 'node:util';
import type { FullConfig } from '@playwright/test';
import { settings } from './environment';
import { tc117Fixture, type Visitor } from './visitor-abuse-environment';
import { TC117Client } from './visitor-abuse-client';
import { validatePrerequisites } from './visitor-abuse-scenarios';
import { prepareEvidenceDirectory } from './security';

export function assertFirstAttempt(
  config: Pick<FullConfig, 'projects'>,
  args = process.argv.slice(2),
) {
  const { values } = parseArgs({
    args,
    options: { retries: { type: 'string' }, 'repeat-each': { type: 'string' } },
    strict: false,
    allowPositionals: true,
  });
  if (
    (values.retries !== undefined && values.retries !== '0') ||
    (values['repeat-each'] !== undefined && values['repeat-each'] !== '1') ||
    config.projects.some((project) => project.retries !== 0 || project.repeatEach !== 1)
  )
    throw new Error('Invalid E2E TC-117 retry/repetition configuration');
}

export function selectedVisitors(
  config: Pick<FullConfig, 'projects'>,
  args = process.argv.slice(2),
): Visitor[] {
  // Playwright's --project is variadic; accept both repeated flags and space-separated names.
  const patterns: string[] = [];
  for (let index = 0; index < args.length; index++) {
    const argument = args[index]!;
    if (argument !== '--project' && !argument.startsWith('--project=')) continue;
    if (argument.startsWith('--project=')) patterns.push(argument.slice('--project='.length));
    while (index + 1 < args.length && !args[index + 1]!.startsWith('-'))
      patterns.push(args[++index]!);
  }
  const projects = config.projects.filter(
    (project) =>
      !patterns.length ||
      patterns.some((pattern) => {
        const escaped = pattern
          .split('*')
          .map((part) => part.replace(/[^a-z0-9_-]/gi, '\\$&'))
          .join('.*');
        return new RegExp(`^${escaped}$`, 'i').test(project.name);
      }),
  );
  const visitors: Visitor[] = projects.some((project) => project.name === 'tc117')
    ? ['boundary', 'a', 'b', 'c']
    : [];
  if (projects.some((project) => project.name === 'tc117-utc')) visitors.push('utc');
  return visitors;
}

export default async function preflight(config: FullConfig) {
  assertFirstAttempt(config);
  const settingsValue = settings();
  const fixture = tc117Fixture();
  const complete = prepareEvidenceDirectory(
    settingsValue.output,
    process.env.CMA_E2E_EVIDENCE_RUN ?? '',
  );
  const visitors = selectedVisitors(config);
  const client = await TC117Client.open(settingsValue, fixture);
  try {
    await validatePrerequisites(client, visitors);
  } finally {
    await client.close(false);
  }
  complete();
  client.writeEvidence('prerequisites', 'validated', process.env.CMA_E2E_EVIDENCE_RUN ?? '');
  if (process.env.GITHUB_OUTPUT) appendFileSync(process.env.GITHUB_OUTPUT, 'evidence_ready=true\n');
}
