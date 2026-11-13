"""Conservative read-only V1 ledger gate for an unused restored process scope.

This is an admission exclusion, not a repair or an alternative runtime reader.
The supervisor still validates its own receipts before every effect. Only a
small, entirely completed persistent ledger and an empty current process scope
qualify. Indexed/partial formats and historical errors require separate proof.
Concatenate after recovery_io.py; no filesystem mutation is permitted here.
"""
LEDGER_LIMIT = 128
LEDGER_BYTES = 16 * 1024 * 1024
LEDGER_IDENTITY = {"operation_id", "request_sha256", "payload_sha256", "subject"}


def ledger_files(root, folder, maximum):
    path = root + "/" + folder
    try:
        with directory(path) as fd:
            names = sorted(os.listdir(fd))
    except FileNotFoundError:
        return []
    require(len(names) <= LEDGER_LIMIT, "ledger_capacity")
    result = []
    total = 0
    for name in names:
        # Temporary writes, format barriers and noncanonical files all hold.
        require(re.fullmatch(r"[0-9a-f]{64}\.json", name), "ledger_unproved")
        data = read(path + "/" + name, maximum)
        total += len(data)
        require(total <= LEDGER_BYTES, "ledger_capacity")
        result.append((name, data, json.loads(data)))
    return result


def ledger_identity(record, runtime):
    require(isinstance(record, dict) and record.get("version") == 1
            and record.get("sandbox_id") == runtime["sandboxId"]
            and record.get("runtime_generation") is None, "ledger_unproved")
    require(isinstance(record.get("operation_id"), str)
            and 0 < len(record["operation_id"]) <= 256, "ledger_unproved")
    for key in ("request_sha256", "payload_sha256"):
        require(isinstance(record.get(key), str)
                and re.fullmatch(r"[0-9a-f]{64}", record[key]), "ledger_unproved")
    return {key: record[key] for key in LEDGER_IDENTITY}


def ledger_resource(record):
    subject = record["subject"]
    require(isinstance(subject, dict), "ledger_unproved")
    kind = subject.get("kind")
    if kind == "create_agent_home":
        require(set(subject) == {"kind", "agent_id"}, "ledger_unproved")
        require(record["operation_id"] == "agent:" + subject["agent_id"] + ":home:create:v1", "ledger_unproved")
        return {"kind": "agent_home", "agent_id": subject["agent_id"]}
    if kind == "add_worktree":
        require(set(subject) == {"kind", "workspace", "name", "base"}, "ledger_unproved")
        require(record["operation_id"] == "worktree:" + subject["workspace"] + ":" + subject["name"] + ":add:v1", "ledger_unproved")
        return {"kind": "worktree", "workspace": subject["workspace"], "name": subject["name"]}
    if kind == "install_agent_skill":
        require(set(subject) == {"kind", "request"} and isinstance(subject["request"], dict), "ledger_unproved")
        request = subject["request"]
        require(request.get("operation_id") == record["operation_id"]
                and request.get("request_sha256") == record["request_sha256"], "ledger_unproved")
        return {"kind": "agent_skill", "agent_id": request["agent_id"], "skill_id": request["skill"]["id"]}
    # No replay of any process or cleanup operation in this first slice.
    raise Refused("ledger_unproved")


def ledger_authority_name(record):
    resource = record["resource"]
    keys = {"worktree": ("workspace", "name"), "agent_home": ("agent_id",),
            "agent_skill": ("agent_id", "skill_id")}[resource["kind"]]
    value = b"chrono.mutation.authority.record.v1\0" + resource["kind"].encode() + b"\0"
    for key in keys:
        field = resource[key].encode()
        value += len(field).to_bytes(8, "big") + field
    value += record["authority_fence"].to_bytes(8, "big") + b"\x01"
    return sha(value) + ".json"


def idle_ledger(runtime):
    root = STATE + "/" + runtime["sandboxId"]
    with directory(root) as fd:
        names = os.listdir(fd)
        require(len(names) <= 4096, "ledger_capacity")
        require(not any(name.startswith(("mutation-ledger-v2", ".mutation-ledger-v2"))
                        for name in names), "ledger_format_unproved")
        space = os.fstatvfs(fd)
        require(space.f_bavail * space.f_frsize >= 64 * 1024 * 1024, "ledger_capacity")
    current = root + "/process-generation-" + str(runtime["generation"])
    # A completed OpenAgent with no live bridge is terminal too: allocating a
    # different operation key would bypass its original uncertain outcome.
    require(not ledger_files(current, "runtime-mutations", 1024 * 1024)
            and not ledger_files(current, "mutation-authority", 64 * 1024), "ledger_process_scope_used")
    journals = ledger_files(root, "runtime-mutations", 1024 * 1024)
    authorities = ledger_files(root, "mutation-authority", 64 * 1024)
    require(len(journals) + len(authorities) <= LEDGER_LIMIT, "ledger_capacity")
    require(sum(len(data) for _, data, _ in journals + authorities) <= LEDGER_BYTES, "ledger_capacity")
    retained = {}
    base = {"version", "sandbox_id"} | LEDGER_IDENTITY
    for name, _, record in journals:
        identity = ledger_identity(record, runtime)
        require(set(record) <= base | {"runtime_generation", "status", "receipt", "failure"}
                and record.get("status") == "completed" and record.get("failure") is None
                and isinstance(record.get("receipt"), dict), "ledger_unsettled")
        require(name == sha(record["operation_id"].encode()) + ".json", "ledger_unproved")
        ledger_resource(record)
        receipt = record["receipt"]
        require(receipt.get("operation_id") == record["operation_id"]
                and receipt.get("request_sha256") == record["request_sha256"], "ledger_unproved")
        retained[record["operation_id"]] = identity
    resources = {}
    authorized = set()
    for name, _, record in authorities:
        identity = ledger_identity(record, runtime)
        require(set(record) <= base | {"runtime_generation", "authority_fence", "resource", "disposition"}
                and record.get("disposition") == "present"
                and type(record.get("authority_fence")) is int
                and 0 < record["authority_fence"] < 2 ** 64, "ledger_unproved")
        require(record.get("resource") == ledger_resource(record)
                and retained.get(record["operation_id"]) == identity, "ledger_unsettled")
        require(name == ledger_authority_name(record), "ledger_unproved")
        key = encode(record["resource"])
        require(key not in resources or resources[key] == identity, "ledger_unproved")
        resources[key] = identity
        authorized.add(record["operation_id"])
    require(authorized == set(retained), "ledger_unsettled")
    # At most 128 persistent rows, no authority-only reservation, and at most
    # 16 new process opens leave >700 journal and >3500 authority slots even
    # with cleanup reservations. This deliberately under-admits the V1 limits.
    fingerprint = [(folder, name, sha(data)) for folder, rows in
                   (("journal", journals), ("authority", authorities)) for name, data, _ in rows]
    return {"format": "legacy_v1_unused_process_scope", "sha256": sha(encode(fingerprint)),
            "journals": len(journals), "authorities": len(authorities)}
