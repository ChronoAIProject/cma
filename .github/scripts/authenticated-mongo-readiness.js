// One bounded probe of the final authenticated replica-set process. The Docker
// entrypoint also starts an authenticated temporary server while creating root;
// successful authentication alone does not mean that final mongod is ready.
(async () => {
const replicaName = 'cma-authenticated-contract';
await db.auth(process.env.MONGO_INITDB_ROOT_USERNAME, process.env.MONGO_INITDB_ROOT_PASSWORD);
const users = (await db.runCommand({connectionStatus: 1})).authInfo?.authenticatedUsers;
if (users?.length !== 1 || users[0].user !== process.env.MONGO_INITDB_ROOT_USERNAME
    || users[0].db !== 'admin') quit(1);
const options = await db.adminCommand({getCmdLineOpts: 1});
// MongoDB 8 reports CLI --replSet as replication.replSet; YAML/--replSetName
// uses replication.replSetName. The temporary Docker bootstrap has neither.
const replication = options.parsed?.replication;
const configuredNames = [replication?.replSet, replication?.replSetName]
  .filter(value => value !== undefined);
if (options.ok !== 1 || configuredNames.length === 0
    || configuredNames.some(value => value !== replicaName)) quit(1);

async function configuration() {
  try {
    // mongosh commands reject asynchronously. Await inside the try so an
    // uninitialized final server reaches the one permitted initialization.
    return await db.adminCommand({replSetGetConfig: 1});
  } catch (error) {
    if (error.code === 94) return {ok: 0, code: 94};
    throw error;
  }
}
let current = await configuration();
if (current.ok !== 1 && current.code === 94) {
  // A lost initialization response is resolved by reading the configuration on
  // the next probe. Never reconfigure or force-init an already initialized set.
  const initialized = await db.adminCommand({replSetInitiate: {
    _id: replicaName, members: [{_id: 0, host: '127.0.0.1:27017'}],
  }});
  if (initialized.ok !== 1) quit(1);
  current = await configuration();
}
if (current.ok !== 1 || current.config?._id !== replicaName
    || current.config.members?.length !== 1
    || current.config.members[0]._id !== 0
    || current.config.members[0].host !== '127.0.0.1:27017') quit(1);
const hello = await db.hello();
if (!hello.isWritablePrimary || hello.setName !== replicaName) quit(1);
quit(0);
})();
