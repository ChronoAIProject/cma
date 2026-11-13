import { readFileSync } from 'node:fs';
import { expect, it } from 'vitest';
import { ProtocolError } from '../src/errors.js';
import * as Gateway from '../src/cmaeg/types.js';

const examples = JSON.parse(
  readFileSync(
    new URL('../../../api/crates/cmaeg/tests/fixtures/sdk-contract.json', import.meta.url),
    'utf8',
  ),
) as Record<string, readonly unknown[]>;

it('preserves GitHub wire examples, including absent fields and explicit nulls', () => {
  const names = [
    'AppIntegration',
    'AppList',
    'AppVerification',
    'BindRequest',
    'Binding',
    'BindingApproval',
    'VerifyBindingRequest',
    'RetireAppRequest',
    'CreateAppRequest',
    'CredentialStatus',
    'GitHubCapabilities',
    'Receipt',
    'Task',
    'TaskResult',
    'TaskResultStatus',
    'UpdateAppRequest',
    'VerifyAppRequest',
    'WebhookCredentialOverlap',
    'WebhookCredentialStatus',
    'WebhookSecretRequest',
    'Source',
    'SourceRequest',
    'SourceKind',
    'SourceId',
    'ProblemCode',
  ] as const;
  for (const name of names) {
    const values = examples[name] ?? [];
    expect(values.length, name).toBeGreaterThan(0);
    for (const value of values) expect(Gateway[`${name}Schema`](value)).toEqual(value);
  }
});

it('preserves explicit null for nullable GitHub object fields', () => {
  const app = {
    ...Gateway.AppIntegrationSchema(examples.AppIntegration?.[0]),
    verification: null,
  };
  const task = { ...Gateway.TaskSchema(examples.Task?.[0]), result: null };
  const credential = {
    ...Gateway.WebhookCredentialStatusSchema(examples.WebhookCredentialStatus?.[0]),
    previous: null,
  };
  expect(Gateway.AppIntegrationSchema(app)).toEqual(app);
  expect(Gateway.TaskSchema(task)).toEqual(task);
  expect(Gateway.WebhookCredentialStatusSchema(credential)).toEqual(credential);
});

it('rejects timestamps that cannot be represented as safe integers', () => {
  const verification = Gateway.AppVerificationSchema(examples.AppVerification?.[0]);
  for (const verified_at of ['1700000000', 1.5, Number.MAX_SAFE_INTEGER + 1, null]) {
    expect(() => Gateway.AppVerificationSchema({ ...verification, verified_at })).toThrow(
      ProtocolError,
    );
  }
});

it('keeps webhook secrets in request shapes and out of integration metadata', () => {
  const request = Gateway.WebhookSecretRequestSchema(examples.WebhookSecretRequest?.[0]);
  expect(request.webhook_secret).toBe('fixture-webhook-secret');
  const integration = Gateway.AppIntegrationSchema(examples.AppIntegration?.[0]);
  expect(
    Gateway.AppIntegrationSchema({ ...integration, webhook_secret: request.webhook_secret }),
  ).toEqual(integration);
});

it('accepts additive per-channel dispatch progress on receipts', () => {
  const receipt = {
    ...Gateway.ReceiptSchema(examples.Receipt?.[0]),
    dispatch: [
      {
        channel_id: 'cmaeg1.ch.example',
        status: 'dispatched',
        reason: null,
        admission_id: 'admission-1',
      },
    ],
  };
  expect(Gateway.ReceiptSchema(receipt)).toEqual(receipt);
});
