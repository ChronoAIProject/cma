import { defineConfig } from '@playwright/test';
import agentV2 from './agent-v2.config';

export default defineConfig({ ...agentV2, testMatch: 'agent-recovery-consistency.check.ts' });
