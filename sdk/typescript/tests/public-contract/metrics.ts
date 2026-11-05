import type { Dispatch } from './context.js';
const operations: Record<string, Dispatch> = {};
for (const admin of [false, true]) {
  const base = `/api/v1/${admin ? 'admin/' : ''}usage/metrics/reports`;
  operations[`POST ${base}`] = ({ client, body }) =>
    (admin ? client.adminUsageMetrics : client.usageMetrics).create(body);
  operations[`GET ${base}/{report}`] = ({ client, param }) =>
    (admin ? client.adminUsageMetrics : client.usageMetrics).report(param('report'));
  operations[`GET ${base}/{report}/sandboxes`] = ({ client, param, options }) =>
    (admin ? client.adminUsageMetrics : client.usageMetrics).sandboxes(param('report'), options);
  operations[`GET ${base}/{report}/sandboxes/{sandbox}`] = ({ client, param }) =>
    (admin ? client.adminUsageMetrics : client.usageMetrics).sandbox(
      param('report'),
      param('sandbox'),
    );
  operations[`GET ${base}/{report}/series`] = ({ client, param, options }) =>
    (admin ? client.adminUsageMetrics : client.usageMetrics).series(param('report'), options);
}
operations['GET /api/v1/admin/usage/metrics/reports/{report}/owners'] = ({
  client,
  param,
  options,
}) => client.adminUsageMetrics.owners(param('report'), options);
export const metricsDispatch = operations;
