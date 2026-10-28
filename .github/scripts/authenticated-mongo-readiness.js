// One bounded probe of the final authenticated replica-set process. The Docker
// entrypoint also starts an authenticated temporary server while creating root;
// successful authentication alone does not mean that final mongod is ready.
const replicaName = 'cma-im-contract';
db.auth(process.env.MONGO_INITDB_ROOT_USERNAME, process.env.MONGO_INITDB_ROOT_PASSWORD);
const users = db.runCommand({connectionStatus: 1}).authInfo?.authenticatedUsers;
if (users?.length !== 1 || users[0].user !== process.env.MONGO_INITDB_ROOT_USERNAME
    || users[0].db !== 'admin') quit(1);
const options = db.adminCommand({getCmdLineOpts: 1});
if (options.ok !== 1 || options.parsed?.replication?.replSetName !== replicaName) quit(1);

function configuration() {
  try {
    return db.adminCommand({replSetGetConfig: 1});
  } catch (error) {
    if (error.code === 94) return {ok: 0, code: 94};
    throw error;
  }
}
let current = configuration();
if (current.ok !== 1 && current.code === 94) {
  // A lost initialization response is resolved by reading the configuration on
  // the next probe. Never reconfigure or force-init an already initialized set.
  const initialized = db.adminCommand({replSetInitiate: {
    _id: replicaName, members: [{_id: 0, host: '127.0.0.1:27017'}],
  }});
  if (initialized.ok !== 1) quit(1);
  current = configuration();
}
if (current.ok !== 1 || current.config?._id !== replicaName
    || current.config.members?.length !== 1
    || current.config.members[0]._id !== 0
    || current.config.members[0].host !== '127.0.0.1:27017') quit(1);
const hello = db.hello();
if (!hello.isWritablePrimary || hello.setName !== replicaName) quit(1);
quit(0);
