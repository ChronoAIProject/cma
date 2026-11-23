import assert from 'node:assert/strict';
import { expect, type Page } from '@playwright/test';
import { object } from './environment';
import type { Harness, Launch } from './harness';
import {
  assertConnectionProof,
  assertProviderObservation,
  identityDigest,
  type ConnectionProof,
  type ProviderObservation,
} from '../../.github/scripts/connection-journey-evidence.mjs';
import {
  assertConnectionReply,
  assertFreshConnectionReport,
  connectionCheckPath,
  connectionStartPath,
  readConnectionIntent,
  type ConnectionIntent,
  type ConnectionMode,
  type ConnectionObservationInput,
} from './connection-journey-contract';

export class ConnectionJourney {
  private readonly service;
  private launch?: Launch;
  private before?: ProviderObservation;
  private intent?: ConnectionIntent;
  constructor(
    private readonly h: Harness,
    readonly mode: ConnectionMode,
  ) {
    this.service = h.fixture.connectionJourneys.services[mode];
  }
  get wizardName() {
    return 'Required connections';
  }
  async begin(page: Page, launch: Launch) {
    this.launch = launch;
    assert.equal(launch.state, 'draft');
    await expect(page.getByRole('region', { name: this.wizardName, exact: true })).toBeVisible();
    this.before = await this.observe('before');
  }
  readStart(body: unknown, reply: unknown) {
    assert.ok(this.launch);
    const intent = readConnectionIntent(body, this.launch.id);
    assert.equal(
      intent.service_id,
      this.service.serviceId,
      'Connection mode selected the wrong fixture service',
    );
    assertConnectionReply(reply, intent, this.h.fixture.accounts.visitor.subject);
    if (this.intent) assert.deepEqual(intent, this.intent, 'Connection retry changed intent');
    this.intent = intent;
    return intent;
  }
  async startResult(page: Page) {
    const response = await page.waitForResponse((response) => {
      const url = new URL(response.url());
      return (
        url.origin === this.h.config.baseURL &&
        url.pathname === connectionStartPath &&
        response.request().method() === 'POST'
      );
    });
    assert.ok(response.ok(), 'Expected a successful connection start response');
    return this.readStart(response.request().postDataJSON(), await response.json());
  }
  async verification(page: Page) {
    assert.ok(this.intent);
    const intent = this.intent;
    await page.waitForResponse(async (response) => {
      const url = new URL(response.url());
      if (
        url.origin !== this.h.config.baseURL ||
        url.pathname !== connectionCheckPath ||
        response.request().method() !== 'POST' ||
        !response.ok()
      )
        return false;
      const body = object(response.request().postDataJSON());
      const report = object(await response.json());
      if (report.ready !== true) return false;
      if (body.active === undefined) return false;
      assert.deepEqual(body.target, intent.target);
      assert.deepEqual(
        body.active,
        { service_id: intent.service_id, intent_id: intent.intent_id },
        'Ready response did not verify the original intent',
      );
      assertFreshConnectionReport(report, intent, this.h.fixture.accounts.visitor.subject);
      return true;
    });
  }
  async returnFromSameTab(page: Page): Promise<void> {
    assert.ok(this.launch && this.intent, 'Same-tab return requires an acknowledged Launch intent');
    await expect(page).toHaveURL(
      (url) => url.origin === this.h.config.baseURL && url.pathname === '/connection-return',
    );
    const link = page.getByRole('link', { name: 'Return to CMA', exact: true });
    await expect(link).toBeVisible();
    assert.equal(
      await link.getAttribute('href'),
      `/launches/${encodeURIComponent(this.launch.id)}`,
      'Callback must return to the original Launch',
    );
    assert.ok(
      [null, '_self'].includes(await link.getAttribute('target')),
      'Callback must stay in the original tab',
    );
    await Promise.all([this.verification(page), link.click()]);
  }
  private async observe(phase: 'before' | 'after') {
    assert.ok(this.launch);
    if (phase === 'after') assert.ok(this.intent);
    const input: ConnectionObservationInput = {
      run: this.h.journal.run,
      caseId: this.h.caseId,
      phase,
      mode: this.mode,
      owner: this.h.fixture.accounts.visitor.subject,
      target: { type: 'launch', id: this.launch.id },
      serviceId: this.service.serviceId,
      ...(phase === 'after' ? { intentId: this.intent!.intent_id } : {}),
    };
    return assertProviderObservation(await this.h.operators.observeConnection(input), {
      run: input.run,
      caseId: input.caseId,
      phase,
      mode: input.mode,
      ownerDigest: identityDigest(input.owner),
      targetDigest: identityDigest(input.target),
      serviceDigest: identityDigest(input.serviceId),
      intentDigest: input.intentId === undefined ? null : identityDigest(input.intentId),
      providerRevision: this.service.providerRevision,
      fixtureDigest: this.service.fixtureDigest,
    });
  }
  async finish(page: Page, variant: ConnectionProof['variant']) {
    assert.ok(this.launch && this.intent && this.before);
    const after = await this.observe('after');
    const ready = await this.h.ready(page, this.launch.id);
    assert.equal(ready.id, this.launch.id);
    assert.ok(ready.resolution.sandbox.id);
    const sandbox = await this.h.get<{ owner: string }>(
      page,
      `/api/v1/sandboxes/${ready.resolution.sandbox.id}`,
    );
    assert.equal(
      sandbox.owner,
      this.h.fixture.accounts.visitor.subject,
      'Ready Sandbox changed owner',
    );
    assert.equal(
      this.h.commands.filter((row) => row.path === '/auth/service-authorization/start').length,
      0,
      'Service authorization recovery requires a separate explicit-resume case',
    );
    const commands = this.h.commands.filter(
      (row) =>
        row.path.startsWith('/auth/connections/') ||
        row.path.endsWith('/launches') ||
        /^\/api\/v1\/launches\/[^/]+\/start$/.test(row.path),
    );
    const proof = assertConnectionProof(
      {
        schema: 'cma-connection-journey-v1',
        execution: 'configured_remote_origin',
        scope: 'ordinary_service_connection',
        run: this.h.journal.run,
        caseId: this.h.caseId,
        mode: this.mode,
        variant,
        ownerDigest: this.before.ownerDigest,
        targetDigest: this.before.targetDigest,
        serviceDigest: this.before.serviceDigest,
        intentDigest: identityDigest(this.intent.intent_id),
        providerRevision: this.service.providerRevision,
        fixtureDigest: this.service.fixtureDigest,
        commands,
        launchCreates: 1,
        launchStarts: 1,
        ownerVerified: true,
        sameTab: variant === 'popup_blocked',
        before: this.before,
        after,
      },
      {
        run: this.h.journal.run,
        caseId: this.h.caseId,
        providerRevision: this.service.providerRevision,
      },
    );
    this.h.proveConnection(proof);
  }
}
