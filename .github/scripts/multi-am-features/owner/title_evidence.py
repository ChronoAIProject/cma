"""Observe only the title events and request identity of a synthetic Agent."""
import json
import re


def title_effects(self, agentId=None, title=None):
    if not isinstance(agentId, str) or not re.fullmatch(r'[A-Za-z0-9_-]{1,160}', agentId):
        raise ValueError('agent_id_required')
    if not isinstance(title, str) or not 1 <= len(title) <= 256:
        raise ValueError('fixture_title_required')
    query = json.dumps({'agent_id': agentId, 'record.kind.type': 'title_set', 'record.kind.title': title})
    return self.mongo(f'const rows=d.agent_log.find({query},{{_id:0,seq:1}}).limit(2).toArray();'
                      'print(JSON.stringify(rows.map(row=>Number(row.seq))));')


def title_intent(self, agentId=None):
    if not isinstance(agentId, str) or not re.fullmatch(r'[A-Za-z0-9_-]{1,160}', agentId):
        raise ValueError('agent_id_required')
    query = json.dumps({'_id': agentId, 'title_effect_floor': 2, 'title_effect_custody.version': 1,
                        'title_effect_custody.intent.agent_id': agentId})
    return self.mongo(f'const row=d.agents.findOne({query},{{title_effect_custody:1}});'
                      'const effect=row?.title_effect_custody;'
                      'print(JSON.stringify(effect ? {request_sha256:effect.intent.request_digest,'
                      'accepted:effect.result!=null,turn_sequence:Number(effect.intent.turn_sequence)} : null));')
