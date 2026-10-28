"""Admitted, retryable graceful EOF recovery for pinned Linux Codex processes.

Execute recovery_io.py + this source with python3 -I -c. Inspection is read-only;
quiesce requires the exact inspected plan durably admitted by the caller. It sends
SIGTERM only to ACP Node, allowing native app-server's stdin EOF shutdown. It
never signals a native process or process group, nor invokes lifecycle APIs.
"""
BRIDGES = {
    "4e3fbc3fb4850f57019f4ba955193363c25c1c044445f165e96d7a65dcf64888",
    "ac50122023f18cec80c4ef543dfbcffe8029de4206cb2692c2f5df51341c2c66",
    "84f9670793e2848b6c49e0541a325223239eeabd0e406d9a586dbcd2935cb85c",
}
NATIVES = {
    "cb0a15567e9a60a5820d54b0f6ae86d504dc3805c1eab21a47f70e3eb7b73a40",
    "e23d0be344d2496986c985cd3db61e6f649b1ddd900e6afc1b5aaabbffcbb4e2",
}
LAUNCHER = "134063e133f0b4244fa3b251acf973d4fe4b4aeeacbdc135211bf480f59f1477"
BRIDGE_PATH = "/usr/local/lib/node_modules/@agentclientprotocol/codex-acp/dist/index.js"


def validate(request):
    require(isinstance(request, dict) and request.get("version") == 1, "request_invalid")
    require(request.get("action") in ("inspect", "quiesce"), "request_invalid")
    keys = {"version", "action", "runtime", "agents"}
    if request["action"] == "quiesce":
        keys |= {"operationId", "expectedPlan"}
        require(re.fullmatch(r"sandbox-lifecycle-v1:sbx_[0-9a-f]{32}:[1-9][0-9]{0,19}", request.get("operationId", "")), "request_invalid")
    require(set(request) == keys, "request_invalid")
    runtime = request["runtime"]
    require(set(runtime) == {"sandboxId", "homeIdentity", "generation", "bootId"}, "request_invalid")
    require(re.fullmatch(r"[a-zA-Z0-9_-]{1,128}", runtime["sandboxId"]), "request_invalid")
    require(re.fullmatch(r"[a-zA-Z0-9:_-]{1,256}", runtime["homeIdentity"]), "request_invalid")
    require(type(runtime["generation"]) is int and 0 < runtime["generation"] < 2 ** 64, "request_invalid")
    require(re.fullmatch(r"[0-9a-f]{32}", runtime["bootId"]), "request_invalid")
    agents = request["agents"]
    require(isinstance(agents, list) and 0 < len(agents) <= 16, "request_invalid")
    for agent in agents:
        require(set(agent) == {"agentId", "conversationId"}, "request_invalid")
        require(re.fullmatch(r"agt_[0-9a-f]{32}", agent["agentId"]), "request_invalid")
        require(UUID.fullmatch(agent["conversationId"]), "request_invalid")
    require(len({a["agentId"] for a in agents}) == len(agents), "request_invalid")


def verify_boot(runtime):
    state = STATE + "/" + runtime["sandboxId"]
    writer = json.loads(read(state + "/workspace-removal-writer-v1.json", 4096))
    require(writer == {"sandbox_id": runtime["sandboxId"], "home_identity": runtime["homeIdentity"],
                       "generation": runtime["generation"], "boot_id": runtime["bootId"]}, "boot_changed")
    binding = json.loads(read(state + "/home-binding/binding.json", 4096))
    require(binding.get("version") == 2 and binding.get("sandbox_id") == runtime["sandboxId"]
            and binding.get("home_identity") == runtime["homeIdentity"]
            and binding.get("runtime_generation") == runtime["generation"], "home_changed")


@contextlib.contextmanager
def boot_guard(runtime):
    path = STATE + "/" + runtime["sandboxId"] + "/workspace-removal-writer-v1.lock"
    with regular(path) as fd:
        require(os.fstat(fd).st_size == 0, "boot_lock_invalid")
        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        verify_boot(runtime)
        yield


def history_summary(agent, previous=None):
    files = histories(agent, previous)
    digest = sha(encode([{k: item[k] for k in ("path", "conversationId", "bytes", "sha256")} for item in files]))
    return {**agent, "historySha256": digest, "files": files}


def topology(agent, processes, supervisor):
    home = HOME + "/.cma/agents/" + agent["agentId"] + "/home"
    candidates = []
    for pid in processes:
        try:
            if environment(pid).get(b"CODEX_HOME") == home.encode():
                candidates.append(pid)
        except FileNotFoundError:
            pass
    if not candidates:
        return {"agentId": agent["agentId"], "home": home, "processes": []}
    bridges = [pid for pid in candidates if processes[pid]["ppid"] == supervisor["pid"]]
    require(len(bridges) == 1, "bridge_topology")
    bridge = bridges[0]
    require(os.readlink(f"{PROC}/{bridge}/exe") == "/usr/local/bin/node", "bridge_topology")
    command = read(f"{PROC}/{bridge}/cmdline", 32768, True).rstrip(b"\0").split(b"\0")
    require(len(command) == 2 and os.path.basename(command[0]) == b"node"
            and os.path.realpath(os.fsdecode(command[1])) == BRIDGE_PATH, "bridge_topology")
    bridge_hash = digest_file(BRIDGE_PATH)
    require(bridge_hash in BRIDGES and processes[bridge]["sid"] == bridge, "bridge_unsupported")
    child = [pid for pid, p in processes.items() if p["ppid"] == bridge]
    require(len(child) == 1, "native_topology")
    launcher = child[0]
    require(os.readlink(f"{PROC}/{launcher}/exe") == "/usr/local/bin/node", "launcher_topology")
    command = read(f"{PROC}/{launcher}/cmdline", 32768, True).rstrip(b"\0").split(b"\0")
    require(len(command) == 3 and os.path.basename(command[0]) == b"node" and command[2] == b"app-server", "launcher_topology")
    launcher_path = os.path.realpath(os.fsdecode(command[1]))
    require(launcher_path.startswith("/usr/local/lib/node_modules/") and launcher_path.endswith("/codex/bin/codex.js")
            and digest_file(launcher_path) == LAUNCHER, "launcher_unsupported")
    native = [pid for pid, p in processes.items() if p["ppid"] == launcher]
    require(len(native) == 1, "native_topology")
    native = native[0]
    identity = proc_identity(native)
    require(identity["exeSha256"] in NATIVES and native in candidates, "native_unsupported")
    native_command = read(f"{PROC}/{native}/cmdline", 32768, True).rstrip(b"\0").split(b"\0")
    require(len(native_command) == 2 and native_command[1] == b"app-server", "native_topology")
    tree = descendants([bridge], processes)
    require(set(candidates) <= tree and len(tree) <= 128, "native_topology")
    owned = [proc_identity(pid) for pid in sorted(tree)]
    cwd = os.readlink(f"{PROC}/{native}/cwd")
    require(cwd.startswith(HOME + "/") and "/.worktrees/" in cwd and len(cwd) <= 1024, "cwd_mismatch")
    require(os.readlink(f"{PROC}/{bridge}/cwd") == cwd, "cwd_mismatch")
    return {"agentId": agent["agentId"], "home": home, "cwdSha256": sha(cwd.encode()), "processes": owned,
            "bridgePid": bridge, "nativePid": native, "launcherPid": launcher,
            "bridgeSha256": bridge_hash, "launcherPath": launcher_path,
            "stdin": stdin_owner(native, bridge, processes)}


def inspect(request):
    with boot_guard(request["runtime"]):
        processes = inventory()
        supervisors = []
        for pid in processes:
            try:
                if os.readlink(f"{PROC}/{pid}/exe") == "/usr/local/bin/layer4-runtime":
                    supervisors.append(pid)
            except FileNotFoundError:
                pass
        require(len(supervisors) == 1, "supervisor_identity")
        supervisor = proc_identity(supervisors[0])
        agents = [topology(a, processes, supervisor) for a in request["agents"]]
        plan = {"version": 1, "processesAbsent": False, "runtime": request["runtime"], "agents": request["agents"],
                "supervisor": supervisor, "topology": agents,
                "baseline": [{"pid": p["pid"], "start": p["start"]} for p in processes.values() if p["pid"] != os.getpid()],
                "histories": [history_summary(a) for a in request["agents"]]}
        require(len(encode(plan)) <= 56 * 1024, "plan_limit")
        verify_boot(request["runtime"])
        return {"version": 1, "status": "inspected", "plan": plan, "planSha256": sha(encode(plan))}


def write_journal(parent, name, value):
    data = encode(value)
    require(len(data) <= 128 * 1024, "journal_limit")
    temporary = name + ".tmp-" + str(os.getpid())
    fd = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600, dir_fd=parent)
    try:
        with os.fdopen(fd, "wb") as file:
            file.write(data)
            file.flush()
            os.fsync(file.fileno())
        os.rename(temporary, name, src_dir_fd=parent, dst_dir_fd=parent)
        os.fsync(parent)
    finally:
        try:
            os.unlink(temporary, dir_fd=parent)
        except FileNotFoundError:
            pass


def validate_live(plan, allow_missing):
    processes = inventory()
    require(proc_identity(plan["supervisor"]["pid"]) == plan["supervisor"], "supervisor_changed")
    for item in plan["topology"]:
        for process in item["processes"]:
            current = processes.get(process["pid"])
            if current is None and allow_missing:
                continue
            require(same_process(process, current), "process_changed")
            # Parent may become init when ACP exits; executable and command must not change.
            live = proc_identity(process["pid"])
            require(all(live[k] == process[k] for k in ("pid", "start", "commandSha256", "exeSha256")), "process_changed")
        if item["processes"] and item["bridgePid"] in processes:
            require(proc_identity(item["bridgePid"]) == next(p for p in item["processes"] if p["pid"] == item["bridgePid"]), "bridge_changed")
            require(digest_file(BRIDGE_PATH) == item["bridgeSha256"] and digest_file(item["launcherPath"]) == LAUNCHER, "binary_changed")
            require(stdin_owner(item["nativePid"], item["bridgePid"], processes) == item["stdin"], "stdin_owner_mismatch")
    return processes


def send_eof(plan, item):
    # pidfd pins the exact original process even if it exits/reuses its PID here.
    require(hasattr(os, "pidfd_open") and hasattr(signal, "pidfd_send_signal"), "pidfd_unavailable")
    try:
        fd = os.pidfd_open(item["bridgePid"], 0)
    except ProcessLookupError:
        return
    try:
        with boot_guard(plan["runtime"]):
            current = validate_live(plan, True)
            if item["bridgePid"] not in current:
                return
            signal.pidfd_send_signal(fd, signal.SIGTERM)
    finally:
        os.close(fd)


def final_history(plan):
    result = []
    for agent, previous in zip(plan["agents"], plan["histories"]):
        item = history_summary(agent, previous["files"])
        require(all(not file["openTurns"] for file in item["files"]), "native_turn_unsettled")
        result.append(item)
    return result


def absent(plan, current):
    original = {p["pid"]: p["start"] for p in plan["baseline"]}
    owned = {p["pid"]: p["start"] for a in plan["topology"] for p in a["processes"]}
    require(all(pid not in current or current[pid]["start"] == start for pid, start in owned.items()), "pid_reused")
    if owned.keys() & current.keys():
        return False
    require(all(pid == os.getpid() or original.get(pid) == p["start"] for pid, p in current.items()), "untracked_process")
    return True


def quiesce(request):
    plan = request["expectedPlan"]
    require(plan["version"] == 1 and plan["runtime"] == request["runtime"] and plan["agents"] == request["agents"], "plan_mismatch")
    require(len(encode(plan)) <= 56 * 1024, "plan_limit")
    digest = sha(encode(plan))
    path = HOME + "/.cma/recovery"
    with directory(HOME + "/.cma") as cma:
        try:
            os.mkdir("recovery", 0o700, dir_fd=cma)
            os.fsync(cma)
        except FileExistsError:
            pass
    with directory(path) as parent, regular(path + "/lock", os.O_RDWR | os.O_CREAT) as lock:
        require(os.fstat(lock).st_size == 0, "journal_lock_invalid")
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        name = sha(request["operationId"].encode()) + ".json"
        try:
            journal = json.loads(read(path + "/" + name, 128 * 1024))
            require(journal["operationId"] == request["operationId"] and journal["planSha256"] == digest
                    and journal["plan"] == plan, "journal_mismatch")
        except FileNotFoundError:
            with boot_guard(plan["runtime"]):
                validate_live(plan, True)
                # Validate old prefix again before recording any signal authority.
                for agent, previous in zip(plan["agents"], plan["histories"]):
                    history_summary(agent, previous["files"])
                journal = {"version": 1, "operationId": request["operationId"], "planSha256": digest, "plan": plan, "status": "admitted"}
                write_journal(parent, name, journal)
        if journal["status"] != "quiesced":
            validate_live(plan, True)
            for item in plan["topology"]:
                if item["processes"]:
                    send_eof(plan, item)
        while True:
            verify_boot(plan["runtime"])
            if absent(plan, inventory()):
                break
            time.sleep(0.05)
        with boot_guard(plan["runtime"]):
            require(proc_identity(plan["supervisor"]["pid"]) == plan["supervisor"], "supervisor_changed")
            result = {"version": 1, "status": "quiesced", "planSha256": digest, "histories": final_history(plan)}
            require(absent(plan, inventory()), "process_reappeared")
            require(len(encode(result)) <= LIMIT, "response_limit")
            journal.update(status="quiesced", result=result)
            write_journal(parent, name, journal)
            return result


def main():
    global DEADLINE
    DEADLINE = time.monotonic() + 25
    # Also bound blocking filesystem/proc operations when the container is unhealthy.
    signal.signal(signal.SIGALRM, lambda *_: (_ for _ in ()).throw(Refused("deadline")))
    signal.alarm(25)
    try:
        raw = sys.stdin.buffer.read(LIMIT + 1)
        require(len(raw) <= LIMIT, "request_limit")
        request = json.loads(raw)
        validate(request)
        response = inspect(request) if request["action"] == "inspect" else quiesce(request)
        require(len(encode(response)) <= LIMIT, "response_limit")
    except Refused as error:
        response = {"version": 1, "status": "refused", "code": str(error)}
    except (OSError, ValueError, TypeError, KeyError, IndexError, AttributeError):
        response = {"version": 1, "status": "refused", "code": "evidence_unavailable"}
    signal.alarm(0)
    sys.stdout.buffer.write(encode(response) + b"\n")


if __name__ == "__main__":
    main()
