// Copy into cases/<assigned-author>/; templates are never auto-registered.
export const cases = [{
  id: 'C275:REPLACE_WITH_ASSIGNED_ID',
  scope: 'global',
  steps: ['step_1', 'expected_result'], // Copy exact catalog required_steps.
  fixtures: [],
  async run(ctx) {
    let response;
    await ctx.step('step_1', async () => {
      response = await ctx.request({actor: 'user', am: ctx.am,
        method: 'GET', path: '/api/v1/REPLACE_WITH_REAL_CONTRACT', allowed: [200]});
      ctx.assert(response.status === 200, 'expected_read_status');
    });
    await ctx.step('expected_result', async () => {
      // Assert the complete row-specific contract using actual response/UI state.
      ctx.assert(false, 'replace_template_with_real_assertions');
    });
  },
}];
