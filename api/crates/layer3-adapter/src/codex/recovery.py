"""Admitted, retryable graceful EOF recovery for pinned Linux Codex processes.

Execute recovery_io.py + this source with python3 -I -c. Inspection is read-only;
quiesce requires the exact inspected plan durably admitted by the caller. It sends
SIGTERM only to ACP Node, allowing native app-server's stdin EOF shutdown. It
never signals a native process or process group, nor invokes lifecycle APIs.
"""
BRIDGES = {
    "11c84d6c37f3b2e15020405a2301df3ed8c16ed2d9b05c3a3a43620bba78a7d4",
    "ddb625f66a5a14d94df3512a525c4a674a359e564602c09085769c8b61e2bca1",
    "e26bce27dbdcf4d665e081b4a81762542795e2af6453da2a2f050fab5110f484",
    "8f0c023269496f9ad21aa2958491772fefe65011b9aebaf674c67d2aaa9fbcf4",
    "fc9f74358c6ce5aeda4ac164c1daf83c513fb0f18df4b75f3d20bd0816fc6fa7",
    "113762e08aaa46cbea8ec3710e334024d13f1cc026a1c264130d5e642e0748c4",
    "4e3fbc3fb4850f57019f4ba955193363c25c1c044445f165e96d7a65dcf64888",
    "ac50122023f18cec80c4ef543dfbcffe8029de4206cb2692c2f5df51341c2c66",
    "84f9670793e2848b6c49e0541a325223239eeabd0e406d9a586dbcd2935cb85c",
}
NATIVES = {
    "cb0a15567e9a60a5820d54b0f6ae86d504dc3805c1eab21a47f70e3eb7b73a40",
    "e23d0be344d2496986c985cd3db61e6f649b1ddd900e6afc1b5aaabbffcbb4e2",
}
LAUNCHER = "134063e133f0b4244fa3b251acf973d4fe4b4aeeacbdc135211bf480f59f1477"
LAUNCHER_NATIVES = {
    LAUNCHER: NATIVES,
    "61b0194f3bb6534439c8d26a3ed57d0805f84b884588b761795323eeb92fcf70": {
        "c06a67fd8ac76508dc78bd99ce20c9f317b8d568beda59485f3f4134df9962d5",
        "b32ece31af06440da980b733fb8fefdd1f193439595fda84096f1caaceec543f",
    },
}
BRIDGE_PATH = "/usr/local/lib/node_modules/@agentclientprotocol/codex-acp/dist/index.js"
RELAY_SOURCE = "50b1c236010050ec957ecec7ce6b52cc79edb58e2eaafefd3da7b9043b213032"
RELAY_SOURCES = {
    RELAY_SOURCE,
    "e595835b6314ff0770fdf97817d6920295ea384a5ac3bfdd0476b90c81b79a4f",
}
DESKTOP_SOURCE = "e7eab92f35acef315b47e8cddec219006e3dda409834836a5bcf2eab3ab0aa00"
# Retained Openbox/XFCE/Omarchy homes coexist with newly admitted desktop Machines.
OMARCHY_DESKTOP_SOURCE = "4544b0e5e66adfebc4da6d13a42e79c51133c72488fc8be5d41bf4e64e8948a3"
DESKTOP_SOURCES = {
    DESKTOP_SOURCE, OMARCHY_DESKTOP_SOURCE,
    "6910bdac6a24cc971f95bc0d2d10f19635a225e6a9f04b333d3555b4cec7ed7e",
    "7b290d8bcf9d95de46c43d069d8b1e73def32f55cfc2ede3e04b7720bc6e439e",
    "c03e804c28de395c7501001f390346f2c2ad29349eab2e17cf9929e7902bcf99",
    "b2b88eaf39c2bf6e6018e2a6185d5461cbaabe1f66fa76562c898c24914af609",
    "3127c5d454316efaf7e147665f871825aae2eda00cea250d2f89fd18f351263d",
}
DESKTOP_PATH = "/usr/local/bin/cma-desktop-session"
RUNTIME_PATH = "/usr/local/bin/layer4-runtime"
DESKTOP_GUARDIAN = b"--desktop-generation-guardian"


def relay_command(command):
    argv = command.rstrip(b"\0").split(b"\0")
    return len(argv) == 5 and argv[:4] == [b"/usr/bin/python3", b"-I", b"-u", b"-c"] and sha(argv[4]) in RELAY_SOURCES


def services(processes):
    """Source-pinned transport relays and the supervisor-owned desktop tree."""
    result = []
    for pid, item in processes.items():
        if pid == os.getpid() or item["ppid"] != 0:
            continue
        try:
            if not relay_command(read(f"{PROC}/{pid}/cmdline", 32768, True)):
                continue
            require(os.readlink(f"{PROC}/{pid}/exe") == os.path.realpath("/usr/bin/python3")
                    and b"CODEX_HOME" not in environment(pid)
                    and not any(p["ppid"] == pid for p in processes.values()), "relay_identity")
            result.append(proc_identity(pid))
        except FileNotFoundError:
            pass
    require(len(result) <= 16, "relay_limit")
    return result + desktop_services(processes)


def desktop_command(pid):
    return read(f"{PROC}/{pid}/cmdline", 32768, True).rstrip(b"\0").split(b"\0")


def desktop_services(processes):
    roots = []
    launcher_command = [b"/bin/bash", DESKTOP_PATH.encode()]
    for pid, item in processes.items():
        if item["ppid"] != 1 or item["sid"] != pid:
            continue
        try:
            command = desktop_command(pid)
            launcher = pid
            if command == [RUNTIME_PATH.encode(), DESKTOP_GUARDIAN]:
                # The runtime holds the guardian's private control pipe for this generation.
                require(os.readlink(f"{PROC}/1/exe") == os.readlink(f"{PROC}/{pid}/exe") == RUNTIME_PATH,
                        "desktop_identity")
                stdin = stdin_owner(pid, 1, (1, pid))
                require(stdin["reader"].startswith("pipe:["), "desktop_identity")
                launchers = [child for child, process in processes.items()
                             if process["ppid"] == pid and process["sid"] == pid
                             and desktop_command(child) == launcher_command]
                require(len(launchers) == 1, "desktop_identity")
                launcher = launchers[0]
            elif command != launcher_command:
                continue
            require(os.readlink(f"{PROC}/{launcher}/exe") == os.path.realpath("/bin/bash")
                    and digest_file(DESKTOP_PATH) in DESKTOP_SOURCES, "desktop_identity")
            env = environment(launcher)
            require(env.get(b"DISPLAY") == b":99" and env.get(b"HOME") == HOME.encode()
                    and b"CODEX_HOME" not in env, "desktop_environment")
            root = proc_identity(pid)
            require(all(root[key] == item[key] for key in ("start", "ppid", "sid")), "desktop_changed")
            if launcher != pid:
                child = proc_identity(launcher)
                require(child["start"] == processes[launcher]["start"] and child["ppid"] == child["sid"] == pid,
                        "desktop_changed")
                root["desktopLauncher"] = child
            roots.append({**root, "desktopRoot": True})
        except FileNotFoundError:
            pass
    require(len(roots) <= 1, "desktop_limit")
    if not roots:
        return []
    root = roots[0]
    # The launcher or its guardian acts as the subreaper for desktop helpers.
    tree = descendants([root["pid"]], processes)
    require(len(tree) <= 256, "desktop_limit")
    return roots + [{k: processes[pid][k] for k in ("pid", "start", "ppid", "sid")}
                    for pid in sorted(tree - {root["pid"]})]


def validate_services(plan, processes):
    current = services(processes)
    roots = lambda items: [p for p in items if p.get("desktopRoot")]
    require(roots(current) == roots(plan.get("services", [])), "desktop_changed")
    return {p["pid"] for p in current}


def validate(request):
    require(isinstance(request, dict) and request.get("version") == 1, "request_invalid")
    require(request.get("action") in ("inspect", "inspect_idle", "quiesce"), "request_invalid")
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
    # A retained runtime can outlive its last live Agent. Empty admission still
    # proves this exact boot/home and rejects every unowned process below.
    require(isinstance(agents, list) and len(agents) <= 16, "request_invalid")
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


def native_history_clock(native, home, identity):
    btime = int(re.search(rb"^btime ([0-9]+)$", read(PROC + "/stat", 1048576, True), re.M)[1])
    uptime = float(read(PROC + "/uptime", 128, True).split()[0])
    require(abs(time.time() - uptime - btime) < 3, "process_clock_uncertain")
    started = int((btime + identity["start"] / os.sysconf("SC_CLK_TCK")) * 1000)
    opened = set()
    for fd in os.listdir(f"{PROC}/{native}/fd"):
        try:
            path = os.readlink(f"{PROC}/{native}/fd/{fd}")
            if path.startswith(home + "/sessions/") or path.startswith(home + "/archived_sessions/"):
                require(path.endswith(".jsonl"), "history_descriptor")
                opened.add(path[len(home) + 1:])
        except FileNotFoundError:
            pass
    require(len(opened) <= 128, "history_limit")
    return {"nativeStartEpochMs": started, "openHistoryPaths": sorted(opened)}


def preboot_turns(file, topology):
    started = topology.get("nativeStartEpochMs")
    if type(started) is not int or started <= 60000:
        return []
    cutoff = started - 60000
    return sorted(turn for turn, start in file["openTurnStarts"].items()
                  if start["at"] is not None and start["at"] < cutoff)


def historical_turns(file, topology):
    opened = file["path"] in topology.get("openHistoryPaths", [])
    terminal = file["lastTerminal"]
    return [turn for turn in preboot_turns(file, topology)
            if not opened or (terminal and terminal["at"] is not None
                and terminal["at"] > file["openTurnStarts"][turn]["at"]
                and terminal["ordinal"] > file["openTurnStarts"][turn]["ordinal"])]


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
    launcher_hash = digest_file(launcher_path)
    require(launcher_path.startswith("/usr/local/lib/node_modules/") and launcher_path.endswith("/codex/bin/codex.js")
            and launcher_hash in LAUNCHER_NATIVES, "launcher_unsupported")
    native = [pid for pid, p in processes.items() if p["ppid"] == launcher]
    require(len(native) == 1, "native_topology")
    native = native[0]
    identity = proc_identity(native)
    require(identity["exeSha256"] in LAUNCHER_NATIVES[launcher_hash] and native in candidates, "native_unsupported")
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
            "bridgeSha256": bridge_hash, "launcherPath": launcher_path, "launcherSha256": launcher_hash,
            "stdin": stdin_owner(native, bridge, processes, launcher), **native_history_clock(native, home, identity)}


def inspect(request):
    with boot_guard(request["runtime"]):
        ledger = idle_ledger(request["runtime"]) if request["action"] == "inspect_idle" else None
        processes = inventory()
        retained_services = services(processes)
        service_pids = {p["pid"] for p in retained_services}
        agent_processes = {pid: p for pid, p in processes.items() if pid not in service_pids}
        supervisors = []
        for pid in agent_processes:
            try:
                if os.readlink(f"{PROC}/{pid}/exe") == RUNTIME_PATH:
                    supervisors.append(pid)
            except FileNotFoundError:
                pass
        require(len(supervisors) == 1, "supervisor_identity")
        supervisor = proc_identity(supervisors[0])
        require(supervisor["pid"] == 1 and supervisor["ppid"] == 0, "supervisor_topology")
        agents = [topology(a, agent_processes, supervisor) for a in request["agents"]]
        permitted = {1, os.getpid()} | {p["pid"] for a in agents for p in a["processes"]} | service_pids
        require(set(processes) <= permitted, "unowned_process")
        history = [history_summary(a) for a in request["agents"]]
        for item, tree in zip(history, agents):
            require(set(tree.get("openHistoryPaths", [])) <= {f["path"] for f in item["files"]}, "history_descriptor")
            for file in item["files"]:
                file["historicalOpenTurns"] = historical_turns(file, tree)
        plan = {"version": 1, "processesAbsent": False, "runtime": request["runtime"], "agents": request["agents"],
                "supervisor": supervisor, "topology": agents, "services": retained_services,
                "baseline": [{"pid": p["pid"], "start": p["start"]} for p in processes.values() if p["pid"] != os.getpid()],
                "histories": history}
        if request["action"] == "inspect_idle":
            # This proof authorizes only reconnecting the original conversations.
            # No signaling, journal write, synthetic terminal row or input replay.
            require(all(not a["processes"] for a in agents), "process_reappeared")
            require(all(not f["openTurns"] for h in history for f in h["files"]), "native_turn_unsettled")
            require(proc_identity(supervisor["pid"]) == supervisor, "supervisor_changed")
            require(absent(plan, inventory()), "process_reappeared")
            for agent, previous in zip(request["agents"], history):
                fresh = history_summary(agent, previous["files"])
                require(fresh["historySha256"] == previous["historySha256"], "history_prefix_changed")
                require(all(not f["openTurns"] for f in fresh["files"]), "native_turn_unsettled")
            require(absent(plan, inventory()), "process_reappeared")
            require(idle_ledger(request["runtime"]) == ledger, "ledger_changed")
            plan["idleReopen"] = True
            plan["idleLedger"] = ledger
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
    service_pids = validate_services(plan, processes)
    agent_processes = {pid: p for pid, p in processes.items() if pid not in service_pids}
    require(proc_identity(plan["supervisor"]["pid"]) == plan["supervisor"], "supervisor_changed")
    for item in plan["topology"]:
        for process in item["processes"]:
            current = processes.get(process["pid"])
            if current is None and allow_missing:
                continue
            require(same_process(process, current), "process_changed")
            # Tool children may exec while their admitted native turn is running.
            # We never signal them; preserve their start identity until absent.
            if process["pid"] in (item["bridgePid"], item["launcherPid"], item["nativePid"]):
                live = proc_identity(process["pid"])
                require(all(live[k] == process[k] for k in ("pid", "start", "commandSha256", "exeSha256")), "process_changed")
        if item["processes"] and item["bridgePid"] in processes:
            for original in item["processes"]:
                if original["pid"] in (item["bridgePid"], item["launcherPid"], item["nativePid"]):
                    current = processes.get(original["pid"])
                    require(current is not None and all(current[k] == original[k] for k in ("ppid", "sid")), "native_topology_changed")
            require(proc_identity(item["bridgePid"]) == next(p for p in item["processes"] if p["pid"] == item["bridgePid"]), "bridge_changed")
            require(digest_file(BRIDGE_PATH) == item["bridgeSha256"]
                    and digest_file(item["launcherPath"]) == item.get("launcherSha256", LAUNCHER), "binary_changed")
            require(stdin_owner(item["nativePid"], item["bridgePid"], agent_processes, item["launcherPid"]) == item["stdin"], "stdin_owner_mismatch")
    owned = {p["pid"] for a in plan["topology"] for p in a["processes"]}
    permitted = descendants(owned, agent_processes) | {plan["supervisor"]["pid"], os.getpid()} | service_pids
    require(set(processes) <= permitted, "unowned_process")
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
    require(absent(plan, inventory()), "process_reappeared")
    result = []
    topology = {item["agentId"]: item for item in plan["topology"]}
    for agent, previous in zip(plan["agents"], plan["histories"]):
        item = history_summary(agent, previous["files"])
        prior = {f["path"]: f for f in previous["files"]}
        for file in item["files"]:
            original = prior.get(file["path"])
            historical = set((original or {}).get("historicalOpenTurns", []))
            if original:
                # Cold load opens a previous process's unfinished history but
                # does not run that turn. After the pinned processes are absent,
                # its unchanged preboot start needs no fabricated terminal row.
                # Derive this from the admitted prefix so old journals replay.
                historical.update(preboot_turns(original, topology.get(agent["agentId"], {})))
            require(set(file["openTurns"]) <= historical, "native_turn_unsettled")
            require(all(file["openTurnStarts"][turn] == prior[file["path"]]["openTurnStarts"][turn]
                        for turn in file["openTurns"]), "native_turn_restarted")
            file["historicalOpenTurns"] = sorted(historical.intersection(file["openTurns"]))
        result.append(item)
    return result


def absent(plan, current):
    owned = {p["pid"]: p["start"] for a in plan["topology"] for p in a["processes"]}
    require(all(pid not in current or current[pid]["start"] == start for pid, start in owned.items()), "pid_reused")
    if owned.keys() & current.keys():
        return False
    supervisor = plan["supervisor"]
    relays = validate_services(plan, current)
    require(all(pid == os.getpid() or pid in relays or (pid == supervisor["pid"] and p["start"] == supervisor["start"]) for pid, p in current.items()), "untracked_process")
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
        response = quiesce(request) if request["action"] == "quiesce" else inspect(request)
        require(len(encode(response)) <= LIMIT, "response_limit")
    except Refused as error:
        response = {"version": 1, "status": "refused", "code": str(error)}
    except (OSError, ValueError, TypeError, KeyError, IndexError, AttributeError):
        response = {"version": 1, "status": "refused", "code": "evidence_unavailable"}
    signal.alarm(0)
    sys.stdout.buffer.write(encode(response) + b"\n")


if __name__ == "__main__":
    main()
