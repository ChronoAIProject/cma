import {pathToFileURL} from 'node:url';
import {createSession,exactKeys} from './migration_agent_session.mjs';
import {requireValue} from './migration_agent_observations.mjs';

const reason=error=>/^(?:session|journal|private_campaign|owned_kind|local_kubernetes|namespace|kubeconfig|observer|native|cleanup|journey|immutable|local_origin|ephemeral|bounded|api_status|send_status|provider_control_status)_[a-z0-9_]{1,90}$/.test(error?.message ?? '')
  ? error.message : 'session_command_failed';

export async function* commands(input) {
  let pending=Buffer.alloc(0),total=0;
  for await(const chunk of input) {
    requireValue(Buffer.isBuffer(chunk) || chunk instanceof Uint8Array,'session_binary_input_required');
    total+=chunk.length;
    requireValue(total<=2*1024*1024,'session_input_total_bound');
    pending=Buffer.concat([pending,chunk]);
    let newline;
    while((newline=pending.indexOf(10))>=0) {
      requireValue(newline>0 && newline<=256*1024,'session_line_bound');
      const line=pending.subarray(0,newline);pending=pending.subarray(newline+1);
      let value;
      try {value=JSON.parse(new TextDecoder('utf-8',{fatal:true}).decode(line));}
      catch {throw new Error('session_json_invalid');}
      yield value;
    }
    requireValue(pending.length<=256*1024,'session_line_bound');
  }
  requireValue(pending.length===0,'session_input_truncated');
}

export async function runProtocol({input,write,makeSession=createSession,signal}) {
  let session=null,sequence=0,finished=false,final=null,errorCode=null,interruptFailure=null;
  let interruptPromise=Promise.resolve();
  const onAbort=()=>{
    if(session) interruptPromise=session.interrupt().catch(error=>{interruptFailure=reason(error);});
    // Wake an idle stdin iterator. Stage writes already transmitted still keep
    // their unknown outcome; no process or application write is restarted.
    if(typeof input.destroy==='function') input.destroy();
  };
  signal?.addEventListener('abort',onAbort,{once:true});
  if(signal?.aborted) onAbort();
  try {
    for await(const command of commands(input)) {
      requireValue(!signal?.aborted,'session_interrupted');
      exactKeys(command,['sequence','op','payload']);
      requireValue(Number.isSafeInteger(command.sequence) && command.sequence===sequence
        && sequence<6 && !finished,'session_sequence_invalid');
      if(sequence===0) {
        requireValue(command.op==='initialize','session_initialize_required');
        session=await makeSession(command.payload);
        await write({sequence,op:'initialized',outcome:'SESSION_INITIALIZED'});
      } else if(command.op==='stage') {
        const result=await session.stage(command.payload);
        await write({sequence,op:'stage',result});
      } else if(command.op==='finish') {
        exactKeys(command.payload,[]);
        finished=true;final=await session.finish('explicit_finish');
        await write({sequence,op:'finished',result:final});
      } else throw new Error('session_operation_invalid');
      sequence++;
    }
    requireValue(!signal?.aborted,'session_interrupted');
    requireValue(finished,'session_unfinished_eof');
  } catch(error) {errorCode=signal?.aborted ? 'session_interrupted' : reason(error);}
  finally {
    signal?.removeEventListener('abort',onAbort);
    await interruptPromise;
    if(session && !finished) {
      finished=true;
      try {final=await session.finish(errorCode ?? 'session_unfinished_eof');}
      catch(error) {errorCode=errorCode ?? reason(error);}
    }
    if(errorCode || interruptFailure) {
      await write({op:'failed',reason:errorCode ?? interruptFailure,
        interrupt_failure:interruptFailure,cleanup:final?.cleanup ?? null,
        journal:final?.journal ?? null});
    }
  }
  return {exitCode:errorCode || interruptFailure || final?.outcome!=='SESSION_OBSERVED' ? 1 : 0,final};
}

export async function main() {
  process.umask(0o077);
  const controller=new AbortController();
  const stop=()=>controller.abort();
  process.once('SIGTERM',stop);process.once('SIGINT',stop);
  const timer=setTimeout(stop,3600000);
  try {
    const result=await runProtocol({input:process.stdin,signal:controller.signal,
      write:async value=>new Promise((resolve,reject)=>{
        process.stdout.write(JSON.stringify(value)+'\n',error=>error ? reject(error) : resolve());
      })});
    process.exitCode=result.exitCode;
  } finally {
    clearTimeout(timer);process.removeListener('SIGTERM',stop);process.removeListener('SIGINT',stop);
  }
}
if(process.argv[1] && import.meta.url===pathToFileURL(process.argv[1]).href) {
  main().catch(()=>{process.stderr.write('migration_agent_session_failed\n');process.exitCode=1;});
}
