// A subprocess fixture for the real protocol/journey. No application image runs.
import {runProtocol} from './migration_agent_protocol.mjs';
import {sessionFixture} from './migration_agent_session_fixture.mjs';

const fixture=sessionFixture();
const result=await runProtocol({input:process.stdin,makeSession:async()=>fixture.session,
  write:async value=>new Promise((resolve,reject)=>{
    process.stdout.write(JSON.stringify(value)+'\n',error=>error?reject(error):resolve());
  })});
process.exitCode=result.exitCode;
