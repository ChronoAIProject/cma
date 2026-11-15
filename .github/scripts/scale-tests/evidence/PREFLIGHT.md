# Local cma-scale results

Overall: **BLOCKED**. Environment: local.

| Scenario | Result | Evidence / blocker |
|---|---|---|
| cross_replica | BLOCKED | environment_config_missing |
| am_mid_stream | BLOCKED | environment_config_missing |
| am_mid_command | BLOCKED | environment_config_missing |
| eg_inbound_loss | BLOCKED | environment_config_missing |
| hpa_up_down | BLOCKED | environment_config_missing |
| users_100 | BLOCKED | environment_config_missing |
| deterministic_contract | BLOCKED | environment_config_missing |
| replica_sweep | BLOCKED | environment_config_missing |
| am_101_trials | BLOCKED | environment_config_missing |
| eg_10_trials | BLOCKED | environment_config_missing |
| eg_partition_load | BLOCKED | environment_config_missing |
| performance | BLOCKED | environment_config_missing |

A unit-test pass is not a local-cluster acceptance pass. Failures and missing prerequisites remain in the matrix.

results.json SHA-256: a518fb3eb457301def60618c4757e27b5def027b7e5e160844746f14ab7d80c6

| Criterion / metric | Phase | N | Failed | p50 ms | p95 ms | p99 ms | Delta ms | Delta % | Noise ms | Result |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| scorecard-L15: create | n1 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L15: create | n2 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L15: create | scaling | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L16: create_desktop | n1 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L16: create_desktop | n2 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L16: create_desktop | scaling | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L17: POST /api/v1/agents | n1 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L17: POST /api/v1/agents | n2 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L17: POST /api/v1/agents | scaling | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L18: GET /api/v1/agent-lifecycle/{id} | n1 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L18: GET /api/v1/agent-lifecycle/{id} | n2 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L18: GET /api/v1/agent-lifecycle/{id} | scaling | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L19: GET /api/v1/agents/{id} | n1 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L19: GET /api/v1/agents/{id} | n2 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L19: GET /api/v1/agents/{id} | scaling | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L20: GET /api/v1/sandboxes/{id} | n1 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L20: GET /api/v1/sandboxes/{id} | n2 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L20: GET /api/v1/sandboxes/{id} | scaling | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L21: GET /api/v1/workspaces/{id} | n1 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L21: GET /api/v1/workspaces/{id} | n2 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L21: GET /api/v1/workspaces/{id} | scaling | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L22: GET /api/v1/desktop-capability | n1 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L22: GET /api/v1/desktop-capability | n2 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L22: GET /api/v1/desktop-capability | scaling | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L23: open | n1 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L23: open | n2 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L23: open | scaling | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L24: GET /auth/session | n1 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L24: GET /auth/session | n2 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L24: GET /auth/session | scaling | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L25: POST /auth/connections/check | n1 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L25: POST /auth/connections/check | n2 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L25: POST /auth/connections/check | scaling | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L26: GET /api/v2/ui/task-experience | n1 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L26: GET /api/v2/ui/task-experience | n2 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L26: GET /api/v2/ui/task-experience | scaling | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L27: GET /api/v2/ui/transport | n1 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L27: GET /api/v2/ui/transport | n2 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L27: GET /api/v2/ui/transport | scaling | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L28: GET /api/v1/agents | n1 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L28: GET /api/v1/agents | n2 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L28: GET /api/v1/agents | scaling | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L29: GET /api/v1/agents/{id} | n1 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L29: GET /api/v1/agents/{id} | n2 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L29: GET /api/v1/agents/{id} | scaling | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L30: GET /api/v1/workspaces/{id} | n1 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L30: GET /api/v1/workspaces/{id} | n2 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L30: GET /api/v1/workspaces/{id} | scaling | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L31: GET /api/v1/sandboxes/{id} | n1 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L31: GET /api/v1/sandboxes/{id} | n2 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L31: GET /api/v1/sandboxes/{id} | scaling | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L32: GET /api/v1/resource-access/{kind}/{id} | n1 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L32: GET /api/v1/resource-access/{kind}/{id} | n2 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L32: GET /api/v1/resource-access/{kind}/{id} | scaling | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L33: GET /api/v1/agents/{id}/transcript | n1 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L33: GET /api/v1/agents/{id}/transcript | n2 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L33: GET /api/v1/agents/{id}/transcript | scaling | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L34: GET /api/v1/agents/{id}/usage | n1 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L34: GET /api/v1/agents/{id}/usage | n2 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L34: GET /api/v1/agents/{id}/usage | scaling | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L35: GET /api/v2/agents/{agent_id}/events/snapshot | n1 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L35: GET /api/v2/agents/{agent_id}/events/snapshot | n2 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L35: GET /api/v2/agents/{agent_id}/events/snapshot | scaling | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L36: GET /api/v2/agents/{agent_id}/events/head | n1 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L36: GET /api/v2/agents/{agent_id}/events/head | n2 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L36: GET /api/v2/agents/{agent_id}/events/head | scaling | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L37: GET /api/v2/agents/{agent_id}/events | n1 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L37: GET /api/v2/agents/{agent_id}/events | n2 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L37: GET /api/v2/agents/{agent_id}/events | scaling | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L38: GET /api/v2/agents/{agent_id}/queued-inputs | n1 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L38: GET /api/v2/agents/{agent_id}/queued-inputs | n2 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L38: GET /api/v2/agents/{agent_id}/queued-inputs | scaling | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L39: GET /api/v1/agents/{id}/files/content | n1 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L39: GET /api/v1/agents/{id}/files/content | n2 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L39: GET /api/v1/agents/{id}/files/content | scaling | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L40: GET /api/v1/agents/{id}/models | n1 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L40: GET /api/v1/agents/{id}/models | n2 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L40: GET /api/v1/agents/{id}/models | scaling | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L41: GET /api/v1/agents/{id}/environment | n1 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L41: GET /api/v1/agents/{id}/environment | n2 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L41: GET /api/v1/agents/{id}/environment | scaling | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L42: send | n1 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L42: send | n2 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L42: send | scaling | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L43: interrupt | n1 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L43: interrupt | n2 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L43: interrupt | scaling | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L44: POST /api/v2/agents/{agent_id}/responses | n1 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L44: POST /api/v2/agents/{agent_id}/responses | n2 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L44: POST /api/v2/agents/{agent_id}/responses | scaling | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L45: POST /api/v2/responses | n1 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L45: POST /api/v2/responses | n2 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L45: POST /api/v2/responses | scaling | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L46: GET /api/v2/responses/{id} | n1 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L46: GET /api/v2/responses/{id} | n2 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L46: GET /api/v2/responses/{id} | scaling | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L47: POST /api/v1/agents/{id}/input | n1 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L47: POST /api/v1/agents/{id}/input | n2 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L47: POST /api/v1/agents/{id}/input | scaling | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L48: POST /api/v2/agents/{agent_id}/responses/{response_id}/controls | n1 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L48: POST /api/v2/agents/{agent_id}/responses/{response_id}/controls | n2 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L48: POST /api/v2/agents/{agent_id}/responses/{response_id}/controls | scaling | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L49: GET /api/v2/commands/{command_id} | n1 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L49: GET /api/v2/commands/{command_id} | n2 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L49: GET /api/v2/commands/{command_id} | scaling | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L50: POST /api/v1/agents/{id}/interrupt | n1 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L50: POST /api/v1/agents/{id}/interrupt | n2 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L50: POST /api/v1/agents/{id}/interrupt | scaling | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L51: POST /api/v2/agents/{agent_id}/responses/{response_id}/interrupt | n1 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L51: POST /api/v2/agents/{agent_id}/responses/{response_id}/interrupt | n2 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L51: POST /api/v2/agents/{agent_id}/responses/{response_id}/interrupt | scaling | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L52: usage | n1 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L52: usage | n2 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L52: usage | scaling | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L53: POST /api/v1/admin/usage/metrics/reports | n1 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L53: POST /api/v1/admin/usage/metrics/reports | n2 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L53: POST /api/v1/admin/usage/metrics/reports | scaling | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L54: GET /api/v1/admin/usage/metrics/reports/{report} | n1 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L54: GET /api/v1/admin/usage/metrics/reports/{report} | n2 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L54: GET /api/v1/admin/usage/metrics/reports/{report} | scaling | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L55: GET /api/v1/admin/usage/metrics/reports/{report}/sandboxes | n1 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L55: GET /api/v1/admin/usage/metrics/reports/{report}/sandboxes | n2 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L55: GET /api/v1/admin/usage/metrics/reports/{report}/sandboxes | scaling | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L56: GET /api/v1/admin/usage/metrics/reports/{report}/series | n1 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L56: GET /api/v1/admin/usage/metrics/reports/{report}/series | n2 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L56: GET /api/v1/admin/usage/metrics/reports/{report}/series | scaling | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L57: GET /api/v1/admin/usage/metrics/reports/{report}/owner-allocations | n1 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L57: GET /api/v1/admin/usage/metrics/reports/{report}/owner-allocations | n2 | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
| scorecard-L57: GET /api/v1/admin/usage/metrics/reports/{report}/owner-allocations | scaling | 0 | 0 | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | unmeasured | BLOCKED |
