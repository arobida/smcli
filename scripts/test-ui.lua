-- UI smoke test for smcli in a real Rex terminal: zsh with this build's hooks drives the history and file pickers.
-- Usage: make test-ui (both pickers in parallel), make test-ui-<picker>,
-- or: rex do scripts/test-ui.lua [smcli=PATH] [picker=history|suggest|file|grep|tldr]. Each run has its own HOME and session.
-- Everything runs in a throwaway HOME, database and service runtime; the user's history is never read.
local TERM = "com.superlogical.terminal"
local SID, BLOCK

local function sh(argv, opts)
  local r, err = rex.exec(argv, opts)
  if not r then error(err, 0) end
  if not r.ok then error(table.concat(argv, " ") .. ": " .. r.stderr .. r.stdout, 0) end
  return (r.stdout:gsub("%s+$", ""))
end

local function put(path, text)
  local f = assert(io.open(path, "w"))
  f:write(text)
  f:close()
end

local root = sh({ "git", "rev-parse", "--show-toplevel" })
local smcli = rex.args.smcli or (root .. "/target/debug/smcli")
local picker = rex.args.picker
local PICKERS = { history = 1, suggest = 1, file = 1, grep = 1, tldr = 1 }
if picker and not PICKERS[picker] then error("picker must be one of history, suggest, file, grep, tldr", 0) end
local function wants(name) return not picker or picker == name end
local name = picker and (picker .. " picker") or "all pickers"
local dir = sh({ "mktemp", "-d", "/tmp/smcli-ui.XXXXXX" })
local work = dir .. "/work"
local env = { HOME = dir, SMCLI_DB = dir .. "/db/h.db", SMCLI_RUNTIME_DIR = dir .. "/run", SMCLI_CONFIG = dir .. "/config.json" }

local function screen()
  local r = rex.block.call(TERM, "format", { session_id = SID, block_id = BLOCK, trim = true })
  return r and r.content or ""
end

local function keys(data)
  rex.block.call(TERM, "write", { session_id = SID, block_id = BLOCK, data = data })
end

-- Poll until every plain-text needle shows.
local function expect(needles, timeout)
  local deadline, s = os.time() + (timeout or 20), ""
  repeat
    s = screen()
    local all = true
    for _, n in ipairs(needles) do
      if not s:find(n, 1, true) then all = false end
    end
    if all then return end
    rex.sleep(0.3)
  until os.time() > deadline
  error({ want = needles, screen = s }, 0)
end

-- Enter on a picker row inserts it; the command must not have run until the second Enter.
local function insert_then_run(output)
  rex.sleep(0.5) -- let the picker settle on its results before Enter
  keys("\r")
  rex.sleep(1)
  if screen():find(output, 1, true) then error({ error = "picker ran the command instead of inserting it", screen = screen() }, 0) end
  keys("\r")
  expect({ output })
end

local step = "setup"
local ok, e = pcall(function()
  sh({ "mkdir", "-p", work })
  put(dir .. "/.zshrc", "PROMPT='READY> '\nunsetopt BEEP\neval \"$('" .. smcli .. "' shell-init zsh)\"\n")
  put(work .. "/alpha file.txt", "rex-ui-file-ok\n")
  sh({ "git", "-C", work, "init", "-q" })
  sh({ "git", "-C", work, "add", "." })
  sh({ "git", "-C", work, "-c", "user.name=t", "-c", "user.email=t@e.x", "commit", "-q", "-m", "one" })
  -- The output (rex-ui-42) differs from the command text, so seeing it proves the command ran.
  sh({ smcli, "record", "zsh", work, "0", "echo rex-ui-$((40+2))", "ui-test" }, { env = env })

  local s, err = rex.call("session.create", { label = "smcli-ui-" .. (picker or "all") })
  if err then error(err, 0) end
  SID = s.session_id
  rex.session.attach { session_id = SID }
  local w, werr = rex.session.new_window {
    session_id = SID,
    window_label = "zsh",
    layout = rex.layout.block {
      flavor = TERM .. ".shell",
      options = {
        cwd = work,
        command = { "sh", "-c", 'export HOME="$1" ZDOTDIR="$1" SMCLI_DB="$1/db/h.db" SMCLI_RUNTIME_DIR="$1/run" SMCLI_CONFIG="$1/config.json"; unset SMCLI_SERVICE; exec zsh -i', "sh", dir },
        exit = { on_completion = false },
      },
    },
  }
  if werr then error(werr, 0) end
  BLOCK = w.block_ids[1]
  expect({ "READY>" }, 30) -- trim drops the trailing space

  if wants("history") then
  step = "history picker (Up)"
  keys("rex-ui")
  keys("\27[A")
  expect({ "esc cancel", "echo rex-ui-$((40+2))" })
  insert_then_run("rex-ui-42")
  rex.block.call(TERM, "clear", { session_id = SID, block_id = BLOCK })
  end

  if wants("file") then
  step = "file picker (Ctrl-T)"
  keys("cat ")
  keys("\20") -- Ctrl-T
  expect({ "^T dirs/files" })
  keys("alpha")
  expect({ "1 Results", "▌alpha file.txt" })
  insert_then_run("rex-ui-file-ok")
  rex.block.call(TERM, "clear", { session_id = SID, block_id = BLOCK })
  end

  if wants("suggest") then
  step = "suggestions (Up, then Ctrl-R flips to this directory's suggestions)"
  keys("\27[A")
  expect({ "esc cancel" })
  keys("\18") -- Ctrl-R
  expect({ "suggested @ ", "echo rex-ui-$((40+2))" })
  insert_then_run("rex-ui-42")
  rex.block.call(TERM, "clear", { session_id = SID, block_id = BLOCK })
  end

  if wants("grep") then
  step = "grep picker (Ctrl-G)"
  keys("cat ")
  keys("\7") -- Ctrl-G
  keys("rex-ui-file")
  expect({ "alpha file.txt" }, 40) -- the service indexes the folder first
  insert_then_run("rex-ui-file-ok")
  rex.block.call(TERM, "clear", { session_id = SID, block_id = BLOCK })
  end

  if wants("tldr") then
  step = "command examples (echo @@ Tab)"
  keys("echo @@")
  keys("\t")
  expect({ "esc cancel", "echo" }, 90) -- first use builds the examples index
  rex.sleep(0.5)
  keys("\r")
  rex.sleep(1)
  local s = screen()
  -- The example replaces "echo @@", so the line still starting with echo but without @@ proves it.
  if s:find("esc cancel", 1, true) or s:find("@@", 1, true) or not s:find("READY> echo ", 1, true) then
    error({ error = "example was not inserted on the prompt", screen = s }, 0)
  end
  end
end)

if SID then rex.call("session.destroy", { session_id = SID }) end
rex.exec({ smcli, "service", "stop" }, { env = env })
rex.exec({ "rm", "-rf", dir })
if ok then
  print("ok   smcli UI (" .. name .. ")")
  return
end
if type(e) ~= "table" then e = { error = tostring(e) } end
e.step = step
print("FAIL " .. name .. " at " .. step)
print(rex.json.encode(e, { indent = true }))
rex.stop(1)
