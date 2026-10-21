// Appended to actual Swift DTO declarations by generate.py; no copied contract definitions.
let output = URL(fileURLWithPath: CommandLine.arguments[1])
let encoder = JSONEncoder()
encoder.outputFormatting = [.sortedKeys, .withoutEscapingSlashes]
func write<T: Encodable>(_ filename: String, _ values: [T]) throws {
    var data = Data()
    for value in values { data.append(try encoder.encode(value)); data.append(0x0A) }
    try data.write(to: output.appendingPathComponent(filename))
}

var requests = [ServiceRequest(op: .ping)]
var record = ServiceRequest(op: .record)
record.event = "00112233-4455-6677-8899-aabbccddeeff"
record.command = "printf 'héllo 🌍\\n'\necho done"
record.shell = "zsh"
record.session = "session-1"
record.exitCode = 141
record.startedAt = 1_700_000_000.125
record.endedAt = 1_700_000_001.5
record.nativeFile = "/home/test/.zsh_history"
record.pipestatus = [0, 141]
record.directory = "/repo"
requests.append(record)
var suggest = ServiceRequest(op: .suggest)
suggest.directory = "/repo"
suggest.prefix = "git "
suggest.limit = 3
suggest.session = "session-1"
requests.append(suggest)
var pick = ServiceRequest(op: .pick)
pick.directory = "/repo"
pick.limit = 5
requests.append(pick)
var search = ServiceRequest(op: .search)
search.query = "commit"
search.filter = SearchFilter(program: "git", subcommand: "commit", since: 1_600_000_000.25,
                             before: 1_800_000_000, minRuns: 2, author: "test", directory: "/repo", flag: "--amend")
search.histfile = "/home/test/.history"
search.loginShell = "/bin/zsh"
search.config = "/home/test/config.json"
search.directory = "/repo"
search.limit = 20
requests.append(search)
for op in [ServiceRequest.Operation.context, .files, .grep] {
    var request = ServiceRequest(op: op)
    request.directory = "/repo/sub"
    request.query = "test"
    request.limit = 10
    requests.append(request)
}
var select = ServiceRequest(op: .select)
select.query = "sr"
select.path = "src/main.rs"
select.mode = "files"
select.directory = "/repo"
requests.append(select)
var selectCommand = ServiceRequest(op: .select)
selectCommand.command = "git status"
selectCommand.query = "git"
selectCommand.mode = "search"
selectCommand.config = "/home/test/config.json"
requests.append(selectCommand)
requests.append(ServiceRequest(op: .shutdown))
for value in [0.84551240822557006, 1_700_000_000.1234567, 979267932.8934171, Double.leastNonzeroMagnitude, Double.greatestFiniteMagnitude] {
    var request = ServiceRequest(op: .record)
    request.startedAt = value
    requests.append(request)
}
try write("requests.jsonl", requests)

var responses = [ServiceResponse(ok: true)]
var ping = ServiceResponse(ok: true)
ping.pid = 12345
ping.indexes = [FileIndexInfo(id: 1, root: "/repo"), FileIndexInfo(id: 2, root: "/other")]
ping.build = "/bin/smcli-service|1700000000.12|123456"
responses.append(ping)
var recorded = ServiceResponse(ok: true)
recorded.event = record.event
recorded.duplicate = false
responses.append(recorded)
var picked = ServiceResponse(ok: true)
picked.suggestions = [Suggestion(command: "git status", score: 1.25, runs: 12, runsHere: 3,
                                 lastRun: "2026-01-01T01:02:03Z", lastExitCode: 0, why: ["directory", "picked"])]
picked.recent = ["echo héllo", "printf 'a\\nb'"]
responses.append(picked)
var searched = ServiceResponse(ok: true)
searched.hits = [SearchEntry(command: "git status", runs: 12, lastRun: "2026-01-01T01:02:03Z",
                            lastDirectory: "/repo", lastExitCode: 0, source: "captured", author: "test", intent: "inspect",
                            flags: ["--short"], frecency: 2.5, successes: 10, runsHere: 3, runsProject: 4, why: ["literal"]),
                 SearchEntry(command: "echo native", runs: 1, lastRun: nil, lastDirectory: nil, lastExitCode: nil, source: "native")]
responses.append(searched)
var context = ProjectContext(directory: "/repo/sub")
context.repository = .init(root: "/repo", gitDir: "/repo/.git", commonDir: "/repo/.git", linkedWorktree: false,
                           branch: "main", head: nil, operation: "merge")
context.projects = [.init(root: "/repo", kind: "rust", manifests: ["Cargo.toml"], name: "test", packageManager: "cargo",
                          scripts: ["Cargo.toml": ["build", "test"]])]
context.repoRelative = "sub"
context.projectRelative = "sub"
var status = GitStatus(fetchedAt: 1_700_000_000.125)
status.oid = "abcdef0"
status.branch = "main"
status.upstream = "origin/main"
status.ahead = 2
status.behind = 1
status.staged = 3
status.unstaged = 4
status.untracked = 5
status.conflicted = 6
context.status = status
context.statusPending = true
var contextual = ServiceResponse(ok: true)
contextual.context = context
responses.append(contextual)
var emptyContext = ServiceResponse(ok: true)
emptyContext.context = ProjectContext(directory: "/outside")
responses.append(emptyContext)
var files = ServiceResponse(ok: true)
files.root = "/repo"
files.instance = 1
files.partial = true
files.files = [FileHit(path: "src/main.rs", insert: "../src/main.rs", provenance: ["grep", "picked"], git: " M",
                      line: 42, column: 2, text: "let text = \"héllo\";"),
               FileHit(path: "README.md", insert: "../README.md", provenance: ["files"])]
responses.append(files)
for code in ["bad_request", "version", "unsupported", "busy", "too_large", "timeout", "failed", "future_error"] {
    responses.append(ServiceResponse.failure(code, "Example \(code)."))
}
var empty = ServiceResponse(ok: true)
empty.suggestions = []
empty.recent = []
empty.hits = []
empty.files = []
empty.indexes = []
empty.duplicate = false
empty.partial = false
responses.append(empty)
try write("responses.jsonl", responses)

// Independently verify Rust-emitted values with Swift when supplied as a second argument.
if CommandLine.arguments.count > 2 {
    let rust = URL(fileURLWithPath: CommandLine.arguments[2])
    func check<T: Codable & Equatable>(_ filename: String, _ expected: [T]) throws {
        let lines = try String(contentsOf: rust.appendingPathComponent(filename), encoding: .utf8).split(separator: "\n")
        let actual = try lines.map { try JSONDecoder().decode(T.self, from: Data($0.utf8)) }
        precondition(actual == expected)
    }
    try check("requests.jsonl", requests)
    try check("responses.jsonl", responses)
}

struct DecodingCase: Encodable {
    let name: String
    let kind: String
    let json: String
    let valid: Bool
}
var decoding: [DecodingCase] = []
func checkDecode(_ name: String, _ kind: String, _ json: String) {
    let data = Data(json.utf8)
    let valid = kind == "request"
        ? (try? JSONDecoder().decode(ServiceRequest.self, from: data)) != nil
        : (try? JSONDecoder().decode(ServiceResponse.self, from: data)) != nil
    decoding.append(DecodingCase(name: name, kind: kind, json: json, valid: valid))
}
for (name, json) in [
    ("missing-version", #"{"op":"ping"}"#),
    ("null-version", #"{"v":null,"op":"ping"}"#),
    ("missing-operation", #"{"v":1}"#),
    ("unknown-operation", #"{"v":1,"op":"rm"}"#),
    ("case-sensitive-operation", #"{"v":1,"op":"PING"}"#),
    ("future-version-decodes", #"{"v":2,"op":"ping"}"#),
    ("unknown-field-ignored", #"{"v":1,"op":"ping","future":{"a":1}}"#),
    ("optional-null", #"{"v":1,"op":"search","command":null,"filter":null,"pipestatus":null}"#),
    ("empty-optionals", #"{"v":1,"op":"record","command":"","filter":{},"pipestatus":[]}"#),
    ("integer-boundaries", #"{"v":1,"op":"record","limit":9223372036854775807,"exitCode":-9223372036854775808}"#),
    ("integer-floats", #"{"v":1.0,"op":"record","limit":1e2,"pipestatus":[0.0,-1e0]}"#),
    ("fractional-int", #"{"v":1,"op":"pick","limit":1.5}"#),
    ("integer-overflow", #"{"v":1,"op":"pick","limit":9223372036854775808}"#),
    ("integer-underflow", #"{"v":1,"op":"pick","limit":-9223372036854775809}"#),
    ("wrong-field-type", #"{"v":1,"op":"ping","limit":"1"}"#),
    ("bool-not-int", #"{"v":true,"op":"ping"}"#),
    ("null-in-pipestatus", #"{"v":1,"op":"record","pipestatus":[0,null]}"#),
    ("double-overflow", #"{"v":1,"op":"record","startedAt":1e999}"#),
    ("malformed", "not json"),
    ("truncated", #"{"v":1,"op":"ping""#),
    ("two-values", "{\"v\":1,\"op\":\"ping\"}\n{\"v\":1,\"op\":\"shutdown\"}")
] { checkDecode(name, "request", json) }
for (name, json) in [
    ("missing-response-version", #"{"ok":true}"#),
    ("missing-ok", #"{"v":1}"#),
    ("null-ok", #"{"v":1,"ok":null}"#),
    ("future-response-version-decodes", #"{"v":2,"ok":true}"#),
    ("response-null", #"{"v":1,"ok":true,"hits":null,"build":null}"#),
    ("pid-bounds", #"{"v":1,"ok":true,"pid":2147483647,"instance":-9223372036854775808}"#),
    ("pid-min", #"{"v":1,"ok":true,"pid":-2147483648}"#),
    ("pid-float", #"{"v":1,"ok":true,"pid":1e3}"#),
    ("pid-overflow", #"{"v":1,"ok":true,"pid":2147483648}"#),
    ("pid-underflow", #"{"v":1,"ok":true,"pid":-2147483649}"#),
    ("failure-message-required", #"{"v":1,"ok":false,"error":{"code":"failed"}}"#),
    ("file-provenance-required", #"{"v":1,"ok":true,"files":[{"path":"a","insert":"a"}]}"#),
    ("context-projects-required", #"{"v":1,"ok":true,"context":{"directory":"/","statusPending":false}}"#),
    ("context-default-required", #"{"v":1,"ok":true,"context":{"directory":"/","projects":[]}}"#),
    ("search-defaults-required", #"{"v":1,"ok":true,"hits":[{"command":"ls","runs":1,"source":"native"}]}"#)
] { checkDecode(name, "response", json) }
// Exercise every nonoptional nested key, including fields with Swift initializer defaults.
for (label, original, key) in [
    ("suggestion", try encoder.encode(picked), "suggestions"),
    ("search-entry", try encoder.encode(searched), "hits"),
    ("file-hit", try encoder.encode(files), "files"),
    ("index-info", try encoder.encode(ping), "indexes")
] {
    let object = try JSONSerialization.jsonObject(with: original) as! [String: Any]
    let first = (object[key] as! [[String: Any]])[0]
    for field in first.keys.sorted() {
        var modified = first
        modified.removeValue(forKey: field)
        let json = try JSONSerialization.data(withJSONObject: ["v": 1, "ok": true, key: [modified]], options: .sortedKeys)
        checkDecode("\(label)-without-\(field)", "response", String(decoding: json, as: UTF8.self))
    }
}
let contextObject = try JSONSerialization.jsonObject(with: encoder.encode(context)) as! [String: Any]
for key in ["repository", "status", "projects"] {
    let nested = key == "projects" ? (contextObject[key] as! [[String: Any]])[0] : contextObject[key] as! [String: Any]
    for field in nested.keys.sorted() {
        var modified = nested
        modified.removeValue(forKey: field)
        var changedContext = contextObject
        changedContext[key] = key == "projects" ? [modified] : modified
        let json = try JSONSerialization.data(withJSONObject: ["v": 1, "ok": true, "context": changedContext], options: .sortedKeys)
        checkDecode("\(key)-without-\(field)", "response", String(decoding: json, as: UTF8.self))
    }
}
try write("decoding.jsonl", decoding)
