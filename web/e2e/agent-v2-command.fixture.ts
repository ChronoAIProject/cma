export function commandStatus(started: string, control = false) {
  return {
    object: 'cma.agent.command',
    receipt: {
      command_id: control ? 'cmd_control' : 'cmd_1',
      agent_id: 'agt_contract',
      response_id: control ? null : 'resp_1',
      admitted_at: started,
      agent_order: 1,
      event_cursor: 1,
    },
    status: control ? 'completed' : 'in_progress',
    queue_deadline: started,
    turn_sequence: 1,
    observed_cursor: 1,
    delivery: 'execution_observed',
    failure: null,
  };
}
