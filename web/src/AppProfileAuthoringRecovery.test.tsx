import { act, fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it } from 'vitest';
import {
  AUTHORING_BOUNDS,
  PROFILE_IDS,
  profileAuthoringTransport,
  REPOSITORY,
  REPOSITORY_RESULT,
  RESEARCH_SKILL,
} from './test/profileAuthoring';

afterEach(() => window.history.replaceState({}, '', '/'));

function pendingResponse() {
  let complete!: (response: Response) => void;
  const promise = new Promise<Response>((resolve) => {
    complete = resolve;
  });
  return { promise, complete };
}

function unavailable(code: string) {
  return Response.json({ status: 503, code, detail: 'Read unavailable.' }, { status: 503 });
}

it('keeps a Sandbox draft while bounds load or fail and retries before accepting in-range resources', async () => {
  const test = profileAuthoringTransport();
  const firstBounds = pendingResponse();
  let calls = 0;
  test.intercept(({ url }) =>
    url.pathname.endsWith('/bounds') && calls++ === 0 ? firstBounds.promise : undefined,
  );
  const user = userEvent.setup();
  test.render('research-owner', '/profiles/sandbox-profiles/new');
  await screen.findByText('Loading resource limits');
  fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'Retained bounded setup' } });
  await user.click(screen.getByRole('button', { name: 'Save draft' }));
  expect(test.writes()).toEqual([]);
  expect(screen.getByRole('radio', { name: 'Custom' })).toBeDisabled();
  await act(async () => firstBounds.complete(unavailable('profile_authoring_unavailable')));
  const alert = await screen.findByText('Resource limits are unavailable.');
  expect(screen.getByLabelText('Name')).toHaveValue('Retained bounded setup');
  await user.click(screen.getByRole('button', { name: 'Save draft' }));
  expect(test.writes()).toEqual([]);
  await user.click(within(alert.parentElement!).getByRole('button', { name: 'Try again' }));
  await waitFor(() => expect(screen.getByRole('radio', { name: 'Custom' })).toBeEnabled());
  await user.click(screen.getByRole('radio', { name: 'Custom' }));
  fireEvent.change(screen.getByLabelText('CPU cores'), { target: { value: '5' } });
  await user.click(screen.getByRole('button', { name: 'Save draft' }));
  expect(screen.getByLabelText('CPU cores')).toHaveAttribute('aria-invalid', 'true');
  expect(test.writes()).toEqual([]);
  fireEvent.change(screen.getByLabelText('CPU cores'), { target: { value: '3.5' } });
  await user.click(screen.getByRole('button', { name: 'Save draft' }));
  await screen.findByRole('heading', { name: 'Retained bounded setup' });
  expect(test.records.get(PROFILE_IDS.sandbox)?.draft).toMatchObject({
    cpu_millis: 3500,
    memory_mib: 4096,
  });
  expect(test.unexpected).toEqual([]);
});

it('rejects malformed deployment bounds and aborts its pending read on route departure', async () => {
  const test = profileAuthoringTransport();
  const retry = pendingResponse();
  let calls = 0;
  test.intercept(({ url }) => {
    if (!url.pathname.endsWith('/bounds')) return undefined;
    return calls++ === 0
      ? Response.json({ ...AUTHORING_BOUNDS, min_cpu_millis: 'unknown' })
      : retry.promise;
  });
  const user = userEvent.setup();
  test.render('research-owner', '/profiles/sandbox-profiles/new');
  await screen.findByText('Resource limits are unavailable.');
  fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'No guessed limits' } });
  await user.click(screen.getByRole('button', { name: 'Save draft' }));
  expect(test.writes()).toEqual([]);
  await user.click(screen.getByRole('button', { name: 'Try again' }));
  await waitFor(() => expect(calls).toBe(2));
  const request = test.requests.filter(({ url }) => url.pathname.endsWith('/bounds')).at(-1);
  await user.click(screen.getByRole('button', { name: 'Account menu for research-owner' }));
  await user.click(screen.getByRole('menuitem', { name: 'Profiles' }));
  await screen.findByRole('heading', { name: 'Profiles' });
  expect(request?.signal?.aborted).toBe(true);
  await act(async () => retry.complete(Response.json(AUTHORING_BOUNDS)));
  expect(screen.queryByText('No guessed limits')).not.toBeInTheDocument();
  expect(test.writes()).toEqual([]);
});

it('retries the real service catalogue without discarding other Sandbox fields', async () => {
  const test = profileAuthoringTransport();
  let calls = 0;
  test.intercept(({ url }) =>
    url.pathname.endsWith('/services') && calls++ === 0
      ? unavailable('profile_services_unavailable')
      : undefined,
  );
  const user = userEvent.setup();
  test.render('research-owner', '/profiles/sandbox-profiles/new');
  const error = await screen.findByText('Services could not be loaded.');
  fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'Catalogue retry' } });
  expect(test.writes()).toEqual([]);
  await user.click(within(error.parentElement!).getByRole('button', { name: 'Try again' }));
  await user.click(await screen.findByRole('checkbox', { name: 'GitHub' }));
  expect(screen.getByLabelText('Name')).toHaveValue('Catalogue retry');
  await user.click(screen.getByRole('button', { name: 'Save draft' }));
  await screen.findByRole('heading', { name: 'Catalogue retry' });
  expect(test.records.get(PROFILE_IDS.sandbox)?.draft).toMatchObject({
    nyxid_services: ['github'],
  });
});

it('retries skill search and paging while retaining an exact selected release across queries', async () => {
  const test = profileAuthoringTransport();
  let calls = 0;
  test.intercept(({ url }) => {
    if (!url.pathname.endsWith('/skills')) return undefined;
    if (calls++ === 0) return unavailable('profile_skills_unavailable');
    if (url.searchParams.get('q') === 'empty') return Response.json({ items: [] });
    if (url.searchParams.get('cursor') === 'second')
      return Response.json({ items: [{ id: 'tools', name: 'Tools', versions: ['1.0'] }] });
    return Response.json({ items: [RESEARCH_SKILL], next: 'second' });
  });
  const user = userEvent.setup();
  test.render('research-owner', '/profiles/agent-profiles/new');
  await screen.findByText('Skills could not be loaded.');
  fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'Pinned research' } });
  await user.click(screen.getByRole('button', { name: 'Try again' }));
  await user.click(await screen.findByRole('button', { name: 'Add Research' }));
  await user.click(screen.getByRole('button', { name: 'More skills' }));
  expect(await screen.findByRole('button', { name: 'Add Tools' })).toBeEnabled();
  fireEvent.change(screen.getByLabelText('Search Ornn skills'), { target: { value: 'empty' } });
  await screen.findByText('No matching skills');
  await user.click(screen.getByRole('button', { name: 'Version for Research' }));
  await user.click(screen.getByRole('menuitemradio', { name: '1.1' }));
  expect(screen.queryByRole('textbox', { name: 'Version for Research' })).not.toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: 'Save draft' }));
  await screen.findByRole('heading', { name: 'Pinned research' });
  expect(test.records.get(PROFILE_IDS.agent)?.draft).toMatchObject({
    skills: [{ source: 'ornn', id: 'research', version: '1.1' }],
  });
  expect(test.requests.some(({ url }) => url.searchParams.get('cursor') === 'second')).toBe(true);
});

it('never verifies a changed repository reference from a stale result and retries the exact public resolve request', async () => {
  const test = profileAuthoringTransport();
  const stale = pendingResponse();
  let calls = 0;
  test.intercept(({ url }) => {
    if (!url.pathname.endsWith('/repositories/resolve')) return undefined;
    calls += 1;
    if (calls === 1) return stale.promise;
    if (calls === 2) return unavailable('repository_unavailable');
    return undefined;
  });
  const user = userEvent.setup();
  test.render('research-owner', '/profiles/workspace-profiles/new');
  await screen.findByRole('heading', { name: 'New workspace profile' });
  fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'Verified checkout' } });
  fireEvent.change(screen.getByLabelText('Repository URL'), {
    target: { value: `${REPOSITORY}.git` },
  });
  fireEvent.change(screen.getByLabelText('Branch or tag'), { target: { value: 'main' } });
  await user.click(screen.getByRole('button', { name: 'Verify repository' }));
  await waitFor(() => expect(calls).toBe(1));
  const first = test.requests.find(({ url }) => url.pathname.endsWith('/repositories/resolve'));
  fireEvent.change(screen.getByLabelText('Branch or tag'), { target: { value: 'release' } });
  expect(first?.signal?.aborted).toBe(true);
  await act(async () => stale.complete(Response.json(REPOSITORY_RESULT)));
  expect(screen.queryByText('Repository verified')).not.toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: 'Save draft' }));
  expect(test.writes()).toEqual([]);
  await user.click(screen.getByRole('button', { name: 'Verify repository' }));
  await screen.findByText(
    'The public repository or reference could not be verified. Check the URL and try again.',
  );
  expect(screen.getByLabelText('Branch or tag')).toHaveValue('release');
  await user.click(screen.getByRole('button', { name: 'Verify repository' }));
  await screen.findByText('Repository verified');
  expect(screen.getByLabelText('Repository URL')).toHaveValue(REPOSITORY);
  expect(screen.getByLabelText('Branch or tag')).toHaveValue('release');
  await user.click(screen.getByRole('button', { name: 'Save draft' }));
  await screen.findByRole('heading', { name: 'Verified checkout' });
  expect(test.records.get(PROFILE_IDS.workspace)?.draft).toMatchObject({
    location: { url: REPOSITORY, reference: 'release' },
  });
  expect(
    test.requests
      .filter(({ url }) => url.pathname.endsWith('/repositories/resolve'))
      .map(({ body }) => body),
  ).toEqual([
    { url: `${REPOSITORY}.git`, reference: 'main' },
    { url: `${REPOSITORY}.git`, reference: 'release' },
    { url: `${REPOSITORY}.git`, reference: 'release' },
  ]);
});

it('aborts a stale skill search and never replaces the current catalogue with its late result', async () => {
  const test = profileAuthoringTransport();
  const stale = pendingResponse();
  test.intercept(({ url }) => {
    if (!url.pathname.endsWith('/skills')) return undefined;
    if (url.searchParams.get('q') === '') return stale.promise;
    return Response.json({
      items: [{ id: 'current', name: 'Current research', versions: ['2.0'] }],
    });
  });
  const user = userEvent.setup();
  test.render('research-owner', '/profiles/agent-profiles/new');
  await screen.findByRole('heading', { name: 'New agent profile' });
  await waitFor(() =>
    expect(test.requests.some(({ url }) => url.pathname.endsWith('/skills'))).toBe(true),
  );
  const first = test.requests.find(({ url }) => url.pathname.endsWith('/skills'));
  fireEvent.change(screen.getByLabelText('Search Ornn skills'), { target: { value: 'current' } });
  expect(first?.signal?.aborted).toBe(true);
  await user.click(await screen.findByRole('button', { name: 'Add Current research' }));
  await act(async () => stale.complete(Response.json({ items: [RESEARCH_SKILL] })));
  expect(screen.queryByRole('button', { name: 'Add Research' })).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Version for Current research' })).toHaveTextContent(
    '2.0',
  );
  expect(test.writes()).toEqual([]);
});

it('applies a verified canonical repository URL without replacing draft edits made during the read', async () => {
  const test = profileAuthoringTransport();
  const resolving = pendingResponse();
  test.intercept(({ url }) =>
    url.pathname.endsWith('/repositories/resolve') ? resolving.promise : undefined,
  );
  const user = userEvent.setup();
  test.render('research-owner', '/profiles/workspace-profiles/new');
  await screen.findByRole('heading', { name: 'New workspace profile' });
  fireEvent.change(screen.getByLabelText('Repository URL'), {
    target: { value: `${REPOSITORY}.git` },
  });
  await user.click(screen.getByRole('button', { name: 'Verify repository' }));
  await waitFor(() =>
    expect(test.requests.some(({ url }) => url.pathname.endsWith('/repositories/resolve'))).toBe(
      true,
    ),
  );
  fireEvent.change(screen.getByLabelText('Name'), {
    target: { value: 'Edited during verification' },
  });
  await user.click(screen.getByRole('button', { name: 'Add command' }));
  fireEvent.change(screen.getByLabelText('Setup command 1'), { target: { value: 'npm ci' } });
  await act(async () =>
    resolving.complete(Response.json({ ...REPOSITORY_RESULT, reference: undefined })),
  );
  await screen.findByText('Repository verified');
  expect(screen.getByLabelText('Name')).toHaveValue('Edited during verification');
  expect(screen.getByLabelText('Setup command 1')).toHaveValue('npm ci');
  await user.click(screen.getByRole('button', { name: 'Save draft' }));
  await screen.findByRole('heading', { name: 'Edited during verification' });
  expect(test.records.get(PROFILE_IDS.workspace)?.draft).toMatchObject({
    location: { type: 'git_repository', url: REPOSITORY },
    setup_commands: ['npm ci'],
  });
  expect(test.writes()[0]?.body).not.toHaveProperty('content.location.reference');
});
