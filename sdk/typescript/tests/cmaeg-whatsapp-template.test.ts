import { expect, it } from 'vitest';
import { OutboundMessageSchema, WhatsappTemplateParameterSchema } from '../src/cmaeg/types.js';

it('preserves typed header references and formatted body parameters', () => {
  for (const [type, reference] of [
    ['image', { id: '123456789' }],
    ['video', { link: 'https://media.example/clip.mp4' }],
    ['document', { link: 'https://media.example/report.pdf', filename: 'report.pdf' }],
  ] as const) {
    const message = {
      whatsapp_template: {
        name: 'invoice',
        language: { code: 'en_US' },
        components: [
          { type: 'header', parameters: [{ type, [type]: reference }] },
          {
            type: 'body',
            parameters: [
              {
                type: 'currency',
                currency: { fallback_value: 'USD 10', code: 'USD', amount_1000: 10000 },
              },
              { type: 'date_time', date_time: { fallback_value: 'Tomorrow' } },
            ],
          },
        ],
      },
    };
    expect(OutboundMessageSchema(message)).toEqual(message);
  }
});
it('refuses unknown parameter kinds and inexact or negative currency integers', () => {
  for (const amount_1000 of [-1, 1.5, 9007199254740992]) {
    expect(() =>
      WhatsappTemplateParameterSchema({
        type: 'currency',
        currency: {
          fallback_value: 'one',
          code: 'USD',
          amount_1000,
        },
      }),
    ).toThrow();
  }
  expect(() => WhatsappTemplateParameterSchema({ type: 'payload', payload: 'callback' })).toThrow();
});
