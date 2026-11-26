import type { Dispatch } from './context.js';
const operations: Record<string, Dispatch> = {};
operations['POST /api/v1/admin/usage/metrics/options'] = ({ client, body }) =>
  client.adminUsageMetrics.options(body);
for (const admin of [false, true]) {
  const base = `/api/v1/${admin ? 'admin/' : ''}usage/metrics/reports`;
  const initial = `/api/v1/${admin ? 'admin/' : ''}usage/metrics/initial`;
  operations[`POST ${initial}/reports`] = ({ client, body }) =>
    (admin ? client.adminUsageMetrics : client.usageMetrics).createInitial(body);
  operations[`POST ${initial}/view`] = ({ client, body }) =>
    (admin ? client.adminUsageMetrics : client.usageMetrics).latestInitial(body);
  operations[`POST ${initial}/reports/{report}`] = ({ client, body, param }) =>
    (admin ? client.adminUsageMetrics : client.usageMetrics).initialReport(param('report'), body);
  operations[`GET /api/v1/${admin ? 'admin/' : ''}usage/metrics/view`] = ({ client, options }) =>
    (admin ? client.adminUsageMetrics : client.usageMetrics).latest(options);
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
operations['GET /api/v1/admin/usage/metrics/reports/{report}/owner-allocations'] = ({
  client,
  param,
  options,
}) => client.adminUsageMetrics.ownerAllocations(param('report'), options);
export const metricsDispatch = operations;
