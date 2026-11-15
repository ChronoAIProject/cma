import http from 'k6/http';
import { check, sleep } from 'k6';
import { Trend } from 'k6/metrics';

const am = new Trend('am_readiness_ms', true);
const eg = new Trend('eg_readiness_ms', true);
export const options = {
  vus: 1,
  duration: __ENV.DURATION || '30s',
  thresholds: { checks: ['rate==1'], http_req_failed: ['rate==0'] },
  summaryTrendStats: ['avg', 'med', 'p(95)', 'max'],
};
export default function () {
  for (const [host,path,metric] of [['am.cma-scale.test','/ready',am],['eg.cma-scale.test','/readyz',eg]]) {
    const result = http.get(`${__ENV.BASE_URL}${path}`, {
      headers: { Host: host, ...(__ENV.FAULT === 'delay' ? {'X-Scale-Fault': 'delay'} : {}) },
      tags: { name: host.startsWith('am') ? 'am-readiness' : 'eg-readiness' },
      timeout: '5s',
    });
    metric.add(result.timings.duration);
    check(result, { ready: (r) => r.status === 200 });
  }
  sleep(1);
}
export function handleSummary(data) {
  return { '/results/summary.json': JSON.stringify(data, null, 2) };
}
