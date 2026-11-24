// @vitest-environment node
import { expect, it } from 'vitest';
import { createClient } from '@cma/client';
import { createRestWebClient } from './rest';
it.each([true, false, undefined, 'true'])(
  'requires explicit v2 support independently of v1: %s',
  async (supported) => {
    const sdk = createClient({
      fetch: async () =>
        Response.json({
          openapi: '3.1.0',
          info: { title: 'Synthetic', version: '1' },
          paths: {},
          'x-cma-task-templates-v1': true,
          'x-cma-task-templates-v2': supported,
        }),
    });
    expect(await createRestWebClient(sdk).readTaskTemplateSupport!()).toBe(true);
    expect(await createRestWebClient(sdk).readTaskTemplateV2Support!()).toBe(supported === true);
  },
);

it.each([true, false, undefined, 'true'])(
  'keeps strict v1 capability independently of v2: %s',
  async (supported) => {
    const sdk = createClient({
      fetch: async () =>
        Response.json({
          openapi: '3.1.0',
          info: { title: 'Synthetic', version: '1' },
          paths: {},
          'x-cma-task-templates-v1': supported,
          'x-cma-task-templates-v2': true,
        }),
    });
    expect(await createRestWebClient(sdk).readTaskTemplateSupport!()).toBe(supported === true);
    expect(await createRestWebClient(sdk).readTaskTemplateV2Support!()).toBe(true);
  },
);
