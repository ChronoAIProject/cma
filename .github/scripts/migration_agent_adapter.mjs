import {boundedBody,localOrigin,safeFailure} from './migration_agent_http.mjs';
import {identity,requireValue} from './migration_agent_observations.mjs';

export class LocalProviderControl {
  #origin;#token;#fetch;
  constructor({origin,token,fetcher=fetch}) {
    this.#origin=localOrigin(origin);
    requireValue(typeof token==='string' && /^[\x21-\x7e]{16,4096}$/.test(token),'ephemeral_provider_token_required');
    this.#token=token;this.#fetch=fetcher;
  }
  async #call(method,path,body) {
    try {
      const response=await this.#fetch(this.#origin+path,{method,redirect:'error',signal:AbortSignal.timeout(15000),
        headers:{Authorization:'Bearer '+this.#token,'Content-Type':'application/json'},
        body:body===undefined ? undefined : JSON.stringify(body)});
      const raw=await boundedBody(response,65536);
      requireValue(response.status===200,'provider_control_status_'+response.status);
      const value=JSON.parse(raw);
      requireValue(value && typeof value==='object' && !Array.isArray(value),'provider_control_shape_invalid');
      // The control protocol is finite. Never pass unrelated reflected values
      // (such as request headers) into an evidence sink.
      return Object.fromEntries(['case_id','request_count','completed_effect_count','active_streams',
        'disconnect_count','released','synthetic_effect'].filter(k=>k in value).map(k=>[k,value[k]]));
    } catch(error) {throw safeFailure(error);}
  }
  prepareTurn({caseId,marker}) {
    requireValue(identity(caseId) && typeof marker==='string' && marker.length>=8 && marker.length<=256,
      'provider_case_invalid');
    return this.#call('POST','/__ws4/arm',{case_id:caseId,marker});
  }
  releaseTurn({caseId}) {
    requireValue(identity(caseId),'provider_case_invalid');
    return this.#call('POST','/__ws4/release',{case_id:caseId});
  }
  oracle({caseId}) {
    requireValue(identity(caseId),'provider_case_invalid');
    return this.#call('GET','/__ws4/state?case_id='+encodeURIComponent(caseId));
  }
}

export function nativeAgentAdapter({api,observer,provider}) {
  return {
    reader:()=>observer.reader(),
    async runtimeFor({agentId}) {
      requireValue(identity(agentId),'agent_identity_required');
      const {body}=await api.request('a','GET','/api/v1/agents/'+agentId);
      requireValue(body?.id===agentId && identity(body.sandbox?.id),'runtime_agent_join_invalid');
      return observer.runtimeFor({agentId,sandboxId:body.sandbox.id});
    },
    prepareTurn:args=>provider.prepareTurn(args),releaseTurn:args=>provider.releaseTurn(args),
    oracle:args=>provider.oracle(args),
  };
}
