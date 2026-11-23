// @vitest-environment node
import { expect, it } from 'vitest';
import { restTransport } from '../test/rest';
import { createRestWebClient } from './rest';

it.each([undefined, false, 'true', true])(
  'only advertises explicit title-mode support: %j',
  async (support) => {
    const test = restTransport();
    test.reply({
      openapi: '3.1.0',
      info: { title: 'CMA', version: '1' },
      paths: {},
      ...(support === undefined ? {} : { 'x-cma-agent-title-mode': support }),
    });
    expect(await createRestWebClient(test.sdk).readTaskTitleModeSupport!()).toBe(support === true);
  },
);
