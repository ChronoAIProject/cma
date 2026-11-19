// @vitest-environment node
import { expect, it } from 'vitest';
import { createClient } from '@cma/client';
import { createRestWebClient } from './rest';
it.each([true, false, undefined, 'true'])(
  'requires explicit typed template capability: %s',
  async (supported) => {
    const sdk = createClient({
      fetch: async () =>
        Response.json({
          openapi: '3.1.0',
          info: { title: 'Synthetic', version: '1' },
          paths: {},
          'x-cma-task-templates-v1': supported,
        }),
    });
    expect(await createRestWebClient(sdk).readTaskTemplateSupport!()).toBe(supported === true);
  },
);
