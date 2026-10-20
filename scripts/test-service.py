#!/usr/bin/env python3
"""End-to-end checks for smcli-service: concurrent clients and startup, a killed service, retries, bad input.

Usage: test-service.py <path to smcli>  (smcli-service must sit next to it). Uses a throwaway database and
runtime directory; never touches the user's history or a running service of theirs.
"""
import json
import os
import shutil
import signal
import socket
import sqlite3
import subprocess
import sys
import tempfile
import time
import uuid
from concurrent.futures import ThreadPoolExecutor

smcli = os.path.abspath(sys.argv[1])
# Unix socket paths are limited to ~104 bytes, so keep the runtime directory short.
root = tempfile.mkdtemp(prefix="smcli-svc.", dir="/tmp")
# SMCLI_SERVICE is left unset: starting the service as needed is the default.
env = {k: v for k, v in os.environ.items() if k not in ("SMCLI_SERVICE", "SMCLI_SERVICE_BIN")}
env.update(SMCLI_DB=f"{root}/history.db", SMCLI_RUNTIME_DIR=f"{root}/run", SMCLI_CONFIG=f"{root}/config.json", HOME=root)


def run(*args, check=True, cli=None, **overrides):
    result = subprocess.run([cli or smcli, *args], env=dict(env, **overrides), capture_output=True, text=True, timeout=30)
    if check and result.returncode != 0:
        raise AssertionError(f"smcli {' '.join(args)} failed: {result.stderr}")
    return result


def record(command, directory="/repo", status=0, cli=None, **overrides):
    run("record", "zsh", "session", directory, str(status), "", "", "", command, cli=cli, **overrides)


def service_pid():
    result = run("service", "status", check=False, SMCLI_SERVICE="connect")
    return int(result.stdout.split("pid ")[1].split(")")[0]) if result.returncode == 0 else None


def socket_path():
    return run("service", "status", SMCLI_SERVICE="connect").stdout.split(" at ")[1].split(" for ")[0]


def rows():
    with sqlite3.connect(env["SMCLI_DB"]) as db:
        return db.execute("SELECT count(*) FROM commands").fetchone()[0]


def raw(payload: bytes):
    """Sends bytes straight to the service and returns its parsed reply, or None if it closed silently."""
    with socket.socket(socket.AF_UNIX) as connection:
        connection.settimeout(10)
        connection.connect(socket_path())
        try:
            connection.sendall(payload)
        except BrokenPipeError:
            pass  # The service answers an oversized request and hangs up before reading the rest.
        reply = b""
        while not reply.endswith(b"\n"):
            chunk = connection.recv(65536)
            if not chunk:
                break
            reply += chunk
    return json.loads(reply) if reply else None


def pid_alive(pid):
    try:
        os.kill(pid, 0)
        return True
    except OSError:
        return False


def wait_for(produce, timeout=15):
    """Calls `produce` until it returns something truthy (or the deadline passes) and returns the last value."""
    deadline = time.time() + timeout
    value = produce()
    while not value and time.time() < deadline:
        time.sleep(0.2)
        value = produce()
    return value


def services():
    out = subprocess.run(["pgrep", "-f", f"smcli-service[^ ]* --db {env['SMCLI_DB']}"], capture_output=True, text=True)
    return [int(pid) for pid in out.stdout.split()]


# Answers like a service from before builds were reported: no `build` field, and it exits when asked to shut down.
FAKE_OLD_SERVICE = """
import json, os, socket, sys
path = sys.argv[1]
server = socket.socket(socket.AF_UNIX)
server.bind(path)
server.listen(8)
while True:
    connection, _ = server.accept()
    request = json.loads(connection.makefile().readline())
    connection.sendall((json.dumps({"v": 1, "ok": True, "pid": os.getpid(), "duplicate": False}) + "\\n").encode())
    connection.close()
    if request["op"] == "shutdown":
        os.unlink(path)
        sys.exit(0)
"""

try:
    # Concurrent startup: many clients, no service yet. Exactly one service wins; nothing is lost.
    with ThreadPoolExecutor(32) as pool:
        list(pool.map(lambda i: record(f"echo start {i}"), range(32)))
    assert rows() == 32, rows()
    time.sleep(0.3)
    assert len(services()) == 1, services()
    first = service_pid()
    assert first is not None

    # Concurrent shells against a running service.
    with ThreadPoolExecutor(16) as pool:
        list(pool.map(lambda i: record(f"git commit -m {i % 5}"), range(200)))
    assert rows() == 232, rows()
    suggestions = json.loads(run("suggest", "git", "--json", "--dir", "/repo").stdout)
    assert len(suggestions) == 5 and sum(s["runs"] for s in suggestions) == 200, suggestions

    # Retry without duplicates: the same event ID twice is acknowledged twice, stored once.
    event = str(uuid.uuid4())
    request = {"v": 1, "op": "record", "event": event, "command": "make retry", "shell": "zsh",
               "session": "raw", "directory": "/repo", "exitCode": 0, "startedAt": time.time()}
    line = (json.dumps(request) + "\n").encode()
    first_ack, second_ack = raw(line), raw(line)
    assert first_ack["ok"] and first_ack["duplicate"] is False, first_ack
    assert second_ack["ok"] and second_ack["duplicate"] is True, second_ack
    assert rows() == 233

    # Directory context: served from cache without waiting for Git, then complete once the refresh lands.
    repo = f"{root}/ctxrepo"
    os.makedirs(f"{repo}/pkg/src")
    git = lambda *a: subprocess.run(["git", "-C", repo, "-c", "user.name=t", "-c", "user.email=t@e.x", *a],
                                    check=True, capture_output=True)
    git("init", "-q", "-b", "main")
    open(f"{repo}/pkg/package.json", "w").write('{"name":"pkg","scripts":{"build":"x"}}')
    open(f"{repo}/Makefile", "w").write("all:\n")
    git("add", ".")
    git("commit", "-q", "-m", "one")
    open(f"{repo}/pkg/src/new.txt", "w").write("x")
    real = os.path.realpath(repo)
    begin = time.time()
    answer = raw((json.dumps({"v": 1, "op": "context", "directory": repo + "/pkg/src"}) + "\n").encode())
    assert time.time() - begin < 1, "context must not wait for Git"
    ctx = answer["context"]
    assert answer["ok"] and ctx["repository"]["root"] == real and ctx["repository"]["branch"] == "main", answer
    assert [p["kind"] for p in ctx["projects"]] == ["node", "make"] and ctx["projectRelative"] == "src", ctx
    deadline = time.time() + 10
    while "status" not in ctx and time.time() < deadline:
        time.sleep(0.05)
        ctx = raw((json.dumps({"v": 1, "op": "context", "directory": repo}) + "\n").encode())["context"]
    assert ctx["status"]["untracked"] == 1 and ctx["status"]["branch"] == "main", ctx
    cli = json.loads(run("context", "--json", "--dir", repo).stdout)
    assert cli["repository"]["root"] == real and cli["status"]["untracked"] == 1, cli
    assert json.loads(run("context", "--json", "--dir", "/").stdout).get("repository") is None
    assert raw(b'{"v":1,"op":"context","directory":"relative"}\n')["error"]["code"] == "bad_request"
    assert raw(b'{"v":1,"op":"context"}\n')["error"]["code"] == "bad_request"

    # Malformed, unknown, oversized, wrong-version and unsupported requests get error responses.
    assert raw(b"not json\n")["error"]["code"] == "bad_request"
    assert raw(b'{"v":1,"op":"rm"}\n')["error"]["code"] == "bad_request"
    assert raw(b'{"v":1,"op":"record"}\n')["error"]["code"] == "bad_request"
    assert raw(b'{"v":99,"op":"ping"}\n')["error"]["code"] == "version"
    assert raw(b'{"v":1,"op":"files"}\n')["error"]["code"] == "bad_request"
    assert raw(b'{"v":1,"op":"grep","directory":"/tmp","query":""}\n')["error"]["code"] == "bad_request"
    assert raw(b'{"v":1,"op":"select","directory":"/tmp","path":"../x","mode":"files","query":"x"}\n')["error"]["code"] == "bad_request"
    assert raw(b'{"v":1,"op":"select","directory":"/tmp","path":"/etc/passwd","mode":"files","query":"x"}\n')["error"]["code"] == "bad_request"
    assert raw(b'{"v":1,"op":"suggest","directory":"relative"}\n')["error"]["code"] == "bad_request"
    assert raw(b"x" * (2 << 20) + b"\n")["error"]["code"] == "too_large"
    bad = dict(request, event="NOT-A-UUID")
    assert raw((json.dumps(bad) + "\n").encode())["error"]["code"] == "failed"
    assert service_pid() == first, "bad input must not kill the service"

    # The picker's first draw: directory suggestions plus every command, most recent first.
    picked = raw(b'{"v":1,"op":"pick","directory":"/repo","limit":3}\n')
    assert picked["ok"] and len(picked["suggestions"]) == 3, picked
    assert len(picked["recent"]) == len(set(picked["recent"])) and picked["suggestions"][0]["command"] in picked["recent"]
    assert raw(b'{"v":1,"op":"pick","directory":"relative"}\n')["error"]["code"] == "bad_request"

    # Search runs in the service, which keeps the index open between queries.
    def search(query):
        reply = raw((json.dumps({"v": 1, "op": "search", "query": query, "directory": "/repo", "limit": 5}) + "\n").encode())
        assert reply["ok"], reply
        return [hit["command"] for hit in reply["hits"]]
    assert search("git commit")[0].startswith("git commit -m"), search("git commit")
    assert raw(b'{"v":1,"op":"search","directory":"/repo"}\n')["error"]["code"] == "bad_request"

    # Writes that bypass the service (fallback, import, another tool) show up in its suggestions and search.
    run("record", "zsh", "s", "/repo", "0", "", "", "", "direct write", SMCLI_SERVICE="0")
    found = json.loads(run("suggest", "direct", "--json", "--dir", "/repo").stdout)
    assert [s["command"] for s in found] == ["direct write"], found
    assert search("direct write")[0] == "direct write"
    # So do its own records, and an index rebuilt by another process.
    record("served write")
    assert search("served write")[0] == "served write"
    run("reindex", SMCLI_SERVICE="0")
    assert search("served write")[0] == "served write"
    served = json.loads(run("search", "served write", "--json").stdout)
    assert served[0]["command"] == "served write", served
    assert service_pid() == first

    # Service killed mid-session: the next commands still work (a new service or the direct fallback).
    os.kill(first, signal.SIGKILL)
    time.sleep(0.1)
    record("after crash")
    assert rows() == 236, rows()
    second = service_pid()
    assert second not in (None, first), (first, second)

    # Service unavailable and can't be started: commands fall back to in-process work.
    run("service", "stop")
    run("record", "zsh", "s", "/repo", "0", "", "", "", "no service", SMCLI_SERVICE_BIN="/nonexistent")
    assert rows() == 237
    assert "no service" in run("suggest", "no", "--dir", "/repo", SMCLI_SERVICE_BIN="/nonexistent").stdout
    assert services() == []

    # SMCLI_SERVICE=0 or off never starts a service, and only `connect` refuses to start one too.
    assert services() == []
    for value in ("0", "off", "connect"):
        run("record", "zsh", "s", "/repo", "0", "", "", "", f"service {value}", SMCLI_SERVICE=value)
        assert services() == [], value
    assert rows() == 240, rows()
    # A pipestatus travels with the record through the service.
    run("record", "zsh", "s", "/repo", "1", "0 1", "", "", "", "false | true")
    assert services(), "the default starts the service"
    piped = [r for r in json.loads(run("history", "--json").stdout) if r["command"] == "false | true"]
    assert piped and piped[0]["pipestatus"] == [0, 1], piped
    run("service", "stop")

    # A service left running by an older build is replaced by the current one on the next use.
    copies = f"{root}/bin"
    os.makedirs(copies)
    copy, copy_service = f"{copies}/smcli", f"{copies}/smcli-service"
    shutil.copy(smcli, copy)
    shutil.copy(smcli + "-service", copy_service)

    def service_binary_replaced():
        shutil.copy(copy_service, copy_service + ".new")
        os.utime(copy_service + ".new", (time.time() + 60, time.time() + 60))
        os.replace(copy_service + ".new", copy_service)

    record("copy one", cli=copy)
    old = service_pid()
    assert old is not None
    service_binary_replaced()
    # Connect-only clients use whatever is running; the default replaces it.
    record("copy two", cli=copy, SMCLI_SERVICE="connect")
    assert service_pid() == old
    record("copy three", cli=copy)
    new = service_pid()
    assert new not in (None, old), (old, new)
    assert len(services()) == 1, services()
    for command in ("copy one", "copy two", "copy three"):
        assert command in [r["command"] for r in json.loads(run("history", "--json").stdout)], command
    # A service from before builds were reported (no `build` in its replies) is replaced too.
    socket_file = socket_path()
    run("service", "stop")
    fake = subprocess.Popen([sys.executable, "-c", FAKE_OLD_SERVICE, socket_file])
    deadline = time.time() + 10
    while not os.path.exists(socket_file) and time.time() < deadline:
        time.sleep(0.05)
    record("after old service", cli=copy)
    assert fake.wait(timeout=10) == 0, "the old service was told to shut down"
    assert service_pid() not in (None, fake.pid)
    assert "after old service" in [r["command"] for r in json.loads(run("history", "--json").stdout)]
    run("service", "stop")

    # An idle service exits on its own.
    run("service", "start", SMCLI_SERVICE_IDLE="1")
    deadline = time.time() + 40
    while services() and time.time() < deadline:
        time.sleep(0.5)
    assert services() == [], "idle service should exit"

    # fff file and grep search: at most 2 instances, idle ones dropped after 3 s.
    run("service", "start", SMCLI_FFF_MAX="2", SMCLI_FFF_IDLE="3")

    def ask(op, directory, query="", **extra):
        reply = raw((json.dumps({"v": 1, "op": op, "directory": directory, "query": query, **extra}) + "\n").encode())
        assert reply["ok"], reply
        return reply

    def indexes():
        return raw(b'{"v":1,"op":"ping"}\n')["indexes"]

    def make_repo(name, files):
        path = f"{root}/{name}"
        os.makedirs(path)
        for rel, text in files.items():
            os.makedirs(os.path.dirname(f"{path}/{rel}") or path, exist_ok=True)
            open(f"{path}/{rel}", "w").write(text)
        g = lambda *a: subprocess.run(["git", "-C", path, "-c", "user.name=t", "-c", "user.email=t@e.x", *a],
                                      check=True, capture_output=True)
        g("init", "-q", "-b", "main")
        g("add", ".")
        g("commit", "-q", "-m", "one")
        return os.path.realpath(path)

    awkward = ["a b.txt", "it's.txt", "$HOME.txt", "star*.txt", "-dash.txt", "new\nline.txt", "q\"uote.txt"]
    repo_a = make_repo("fffrepo", {"src/main.swift": "let needle = 1\n", "src/util.swift": "func helper() {}\n",
                                   "README.md": "hello needle\n", **{f"awk/{n}": "awk\n" for n in awkward}})
    open(f"{repo_a}/src/util.swift", "a").write("// edited\n")
    src = f"{repo_a}/src"

    # Repeated searches, from any folder of the worktree, reuse one instance; paths are relative to the root.
    answers = [ask("files", src, "main") for _ in range(5)] + [ask("files", repo_a, "util")]
    assert len({a["instance"] for a in answers}) == 1 and len(indexes()) == 1, (answers[0], indexes())
    assert answers[0]["root"] == repo_a
    top = answers[0]["files"][0]
    assert top["path"] == "src/main.swift" and top["insert"] == "main.swift" and "files" in top["provenance"], top
    assert ask("files", repo_a, "main")["files"][0]["insert"] == "src/main.swift"
    assert ask("files", repo_a + "/awk", "main")["files"][0]["insert"] == "../src/main.swift"
    util = answers[-1]["files"][0]
    assert top.get("git") is None, top
    assert util["path"] == "src/util.swift" and util["git"] == " M", util

    # Content search: line, column and text; plain text, not regex.
    hits = ask("grep", repo_a, "needle")["files"]
    assert {h["path"] for h in hits} == {"src/main.swift", "README.md"}, hits
    main = next(h for h in hits if h["path"] == "src/main.swift")
    assert (main["line"], main["column"], main["text"].strip()) == (1, 5, "let needle = 1"), main
    assert main["provenance"] == ["grep"], main
    assert ask("grep", repo_a, "n.edle")["files"] == []
    assert len(indexes()) == 1

    # Changes become visible without a new instance: new, renamed and deleted files.
    def visible(query, present=True):
        deadline = time.time() + 10
        while time.time() < deadline:
            found = any(h["path"].endswith(query) for h in ask("files", repo_a, query)["files"])
            if found == present:
                return True
            time.sleep(0.2)
        return False
    open(f"{repo_a}/fresh-file.txt", "w").write("x\n")
    assert visible("fresh-file.txt"), "new file should appear"
    os.rename(f"{repo_a}/fresh-file.txt", f"{repo_a}/renamed-file.txt")
    assert visible("renamed-file.txt") and visible("fresh-file.txt", present=False)
    os.remove(f"{repo_a}/renamed-file.txt")
    assert visible("renamed-file.txt", present=False)
    assert len(indexes()) == 1 and ask("files", repo_a, "main")["instance"] == answers[0]["instance"]

    # Awkward names arrive intact (quoting is the adapter's job; see ShellQuotingTests).
    for name in awkward:
        found = [h["insert"] for h in ask("grep", repo_a, "awk")["files"]]
        assert f"awk/{name}" in found, (name, found)

    # An explicit selection is stored (schema 6) and reflected in later results; nothing else writes it.
    with sqlite3.connect(env["SMCLI_DB"]) as db:
        assert db.execute("SELECT count(*) FROM file_selections").fetchone()[0] == 0
    ask("select", src, "util", path="src/util.swift", mode="files")
    with sqlite3.connect(env["SMCLI_DB"]) as db:
        assert db.execute("SELECT root, path, mode, query FROM file_selections").fetchall() == [(repo_a, "src/util.swift", "files", "util")]
        assert db.execute("PRAGMA user_version").fetchone()[0] == 7
    assert "picked" in ask("files", repo_a, "util")["files"][0]["provenance"]

    # Ranking signals through the service: a command that follows the session's last one, then explicit picks.
    plain_dir = os.path.realpath(root)

    def rec(session, command, directory=plain_dir):
        reply = raw((json.dumps({"v": 1, "op": "record", "event": str(uuid.uuid4()), "command": command, "shell": "zsh",
                                 "session": session, "directory": directory, "exitCode": 0}) + "\n").encode())
        assert reply["ok"], reply

    def suggested(prefix, **extra):
        reply = raw((json.dumps({"v": 1, "op": "suggest", "directory": plain_dir, "prefix": prefix, "limit": 5, **extra}) + "\n").encode())
        assert reply["ok"], reply
        return reply["suggestions"]
    for i in range(4):
        rec(f"t{i}", "zzq add")
        rec(f"t{i}", "zzq commit")
    for i in range(6):
        rec(f"u{i}", "zzq status")
    rec("live", "zzq add")
    assert suggested("zzq ")[0]["command"] == "zzq status"
    hit = suggested("zzq ", session="live")[0]
    assert hit["command"] == "zzq commit" and "often follows zzq add" in hit["why"], hit
    for _ in range(3):
        assert ask("select", plain_dir, "zzq ", command="zzq commit", mode="suggest")["ok"]
    for bad in ({"command": "", "mode": "suggest"}, {"command": "x", "mode": "other"}, {"command": "smcli search x", "mode": "search"}):
        assert raw((json.dumps({"v": 1, "op": "select", "directory": plain_dir, "query": "q", **bad}) + "\n").encode())["ok"] is False
    with sqlite3.connect(env["SMCLI_DB"]) as db:
        assert db.execute("SELECT query, command, kind FROM command_selections").fetchall() == [("zzq ", "zzq commit", "suggest")] * 3
    top = suggested("zzq ")[0]
    assert top["command"] == "zzq commit" and top["why"][-1] == "picked for similar searches", top
    # A fresh record is searchable at once (the corpus is updated in place), with no rebuild of the others.
    rec("t9", "zzq freshly recorded thing")
    assert search("zzq freshly recorded thing")[0] == "zzq freshly recorded thing", search("zzq freshly recorded thing")

    # Eviction: at most 2 instances, the least recently used goes first; idle ones are dropped.
    repo_b = make_repo("fffrepo-b", {"b.txt": "b\n"})
    repo_c = make_repo("fffrepo-c", {"c.txt": "c\n"})
    first_id = ask("files", repo_a, "main")["instance"]
    ask("files", repo_b, "b")
    ask("files", repo_a, "main")
    ask("files", repo_c, "c")
    assert sorted(i["root"] for i in indexes()) == sorted([repo_a, repo_c]), indexes()
    assert ask("files", repo_a, "main")["instance"] == first_id
    time.sleep(3.5)
    ask("files", repo_b, "b")
    assert [i["root"] for i in indexes()] == [repo_b], indexes()
    assert ask("files", repo_a, "main")["instance"] != first_id, "an evicted root gets a fresh instance"

    # Stale indexes. The search index is a cache named by version, model and dimension: one from an older version is
    # deleted on the next rebuild, one for another model is kept and reported, and a damaged current one is rebuilt.
    search_dir = env["SMCLI_DB"] + ".search"
    os.makedirs(f"{search_dir}/commands-v6-potion-old-d256")
    open(f"{search_dir}/commands-v6-potion-old-d256/index.pxkt", "w").write("old")
    os.makedirs(f"{search_dir}/commands-v7-potion-othermodel-d256")
    run("reindex", SMCLI_SERVICE="0")
    left = sorted(n for n in os.listdir(search_dir) if n.startswith("commands-v"))
    assert "commands-v6-potion-old-d256" not in left and "commands-v7-potion-othermodel-d256" in left, left
    assert "commands-v7-potion-othermodel-d256" in run("health", SMCLI_SERVICE="0").stdout
    current = next(n for n in left if "othermodel" not in n)
    assert run("health", SMCLI_SERVICE="0", check=False).returncode == 0

    def index_files():
        return [os.path.join(d, f) for d, _, names in os.walk(f"{search_dir}/{current}") for f in names]
    # Damaged under a running service: the service notices the changed files and rebuilds instead of failing.
    assert search("git commit")[0].startswith("git commit -m")
    for path in index_files():
        open(path, "w").write("garbage")
    broken = run("health", SMCLI_SERVICE="0", check=False)
    assert broken.returncode != 0 and "unreadable" in broken.stdout, broken.stdout
    assert search("git commit")[0].startswith("git commit -m"), "search must survive a damaged index"
    healed = run("health", SMCLI_SERVICE="0", check=False)
    assert healed.returncode == 0, healed.stdout
    # Emptied files (a disk-full save) are handled the same way, this time by a client without a service.
    for path in index_files():
        open(path, "w").close()
    assert run("search", "git commit", "-n", "1", SMCLI_SERVICE="0").stdout.startswith("git commit"), "in-process rebuild"
    assert run("health", SMCLI_SERVICE="0", check=False).returncode == 0

    # Suggestion index after external writes: rows changed by another process (here soft delete, as `prune` does, and
    # undo) show up in the service's next suggestion without a restart.
    def suggested_commands(prefix):
        return [s["command"] for s in suggested(prefix)]
    rec("ext", "extwrite one")
    assert suggested_commands("extwrite") == ["extwrite one"]
    with sqlite3.connect(env["SMCLI_DB"]) as db:
        db.execute("UPDATE commands SET deleted_at = 1 WHERE command = 'extwrite one'")
    assert suggested_commands("extwrite") == [], "an externally deleted command must leave suggestions"
    assert "extwrite one" not in search("extwrite one"), "and search"
    with sqlite3.connect(env["SMCLI_DB"]) as db:
        db.execute("UPDATE commands SET deleted_at = NULL WHERE command = 'extwrite one'")
    assert suggested_commands("extwrite") == ["extwrite one"]
    assert pid_alive(raw(b'{"v":1,"op":"ping"}\n')["pid"])

    # Large repository: 50k tracked files in 500 directories plus untracked ones. Nothing may block on Git or the
    # scan, and the answers must stay right. (Timings are printed; the bounds only catch a collapse.)
    big = f"{root}/big"
    for d in range(500):
        os.makedirs(f"{big}/pkg{d // 50}/dir{d}")
        for f in range(100):
            with open(f"{big}/pkg{d // 50}/dir{d}/file{f}.txt", "w") as out:
                out.write(f"line {d} {f}\n")
    open(f"{big}/pkg3/dir150/unique_zebra_marker.swift", "w").write("let marker = 1 // quokka_token_9137\n")
    open(f"{big}/pkg3/package.json", "w").write('{"name":"big","scripts":{"build":"x"}}')
    begin = time.time()
    g = lambda *a: subprocess.run(["git", "-C", big, "-c", "user.name=t", "-c", "user.email=t@e.x", "-c", "gc.auto=0", *a],
                                  check=True, capture_output=True)
    g("init", "-q", "-b", "main")
    g("add", ".")
    g("commit", "-q", "-m", "big")
    print(f"  big repo: 50002 files committed in {time.time() - begin:.1f} s")
    for n in range(5):
        open(f"{big}/pkg0/dir0/untracked{n}.txt", "w").write("u")
    big_real = os.path.realpath(big)
    deep = f"{big}/pkg3/dir150"
    timings = {}
    begin = time.time()
    answer = raw((json.dumps({"v": 1, "op": "context", "directory": deep}) + "\n").encode())
    timings["context first"] = time.time() - begin
    assert answer["ok"] and answer["context"]["repository"]["root"] == big_real, answer
    assert timings["context first"] < 1, "context must not wait for Git, even on a large repository"
    deadline = time.time() + 60
    ctx = answer["context"]
    while "status" not in ctx and time.time() < deadline:
        time.sleep(0.1)
        ctx = raw((json.dumps({"v": 1, "op": "context", "directory": deep}) + "\n").encode())["context"]
    timings["context status"] = time.time() - begin
    assert ctx["status"]["untracked"] == 5 and ctx["status"]["staged"] == 0, ctx
    begin = time.time()
    again = raw((json.dumps({"v": 1, "op": "context", "directory": deep}) + "\n").encode())
    timings["context warm"] = time.time() - begin
    assert again["context"]["status"]["untracked"] == 5 and timings["context warm"] < 0.5

    begin = time.time()
    found = ask("files", deep, "unique_zebra_marker")
    timings["files first"] = time.time() - begin
    assert found["files"][0]["path"] == "pkg3/dir150/unique_zebra_marker.swift", found["files"][:3]
    assert found["files"][0]["insert"] == "unique_zebra_marker.swift"
    begin = time.time()
    found = ask("files", deep, "unique_zebra_marker")
    timings["files warm"] = time.time() - begin
    begin = time.time()
    hits = ask("grep", deep, "quokka_token_9137")["files"]
    timings["grep warm"] = time.time() - begin
    assert [h["path"] for h in hits] == ["pkg3/dir150/unique_zebra_marker.swift"] and hits[0]["line"] == 1, hits
    # A file added after the index was built is found; the instance is still the same one.
    open(f"{big}/pkg7/dir350/late_arrival_file.txt", "w").write("x")
    assert wait_for(lambda: [h for h in ask("files", big, "late_arrival_file")["files"] if h["path"].endswith("late_arrival_file.txt")]), "late file"
    assert max(timings["files warm"], timings["grep warm"]) < 2, timings
    rss = int(subprocess.run(["ps", "-o", "rss=", "-p", str(service_pid())], capture_output=True, text=True).stdout) // 1024
    print("  big repo: " + ", ".join(f"{k} {v * 1000:.0f} ms" for k, v in timings.items()) + f"; service RSS {rss} MB")
    assert rss < 1500, rss
    # The big root is the 3rd distinct root with a pool of 2: the least recently used one went, the service stayed up.
    assert len(indexes()) <= 2, indexes()

    # A directory outside Git is searched as its own root; home and / are refused rather than indexed.
    plain = f"{root}/plain"
    os.makedirs(plain)
    open(f"{plain}/loose.txt", "w").write("x")
    assert ask("files", plain, "loose")["root"] == os.path.realpath(plain)
    assert raw((json.dumps({"v": 1, "op": "files", "directory": "/", "query": "x"}) + "\n").encode())["ok"] is False
    run("service", "stop", check=False)

    print("smcli-service tests passed.")
finally:
    for pid in services():
        os.kill(pid, signal.SIGKILL)
    shutil.rmtree(root, ignore_errors=True)
