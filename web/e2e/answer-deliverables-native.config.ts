import { defineConfig } from '@playwright/test';
import deliveries from './answer-deliverables.config';

const baseURL = process.env.CMA_NATIVE_WEB_URL;
const api = process.env.CMA_NATIVE_DELIVERY_ORIGIN;
if (!baseURL || !api || [baseURL, api].some((value) => new URL(value).hostname !== '127.0.0.1'))
  throw new Error('Native delivery proof requires isolated loopback Web and API servers.');

export default defineConfig({
  ...deliveries,
  testMatch: 'answer-deliverables-native.check.ts',
  use: { ...deliveries.use, baseURL },
});
