#!/usr/bin/env lua
-- Commit with an LLM-written caveman-commit message (agents/reference/commits.md).
-- The rules live here (RULES, check); keep them in step with the runbook and with
-- template-append in .config/wt.toml (used by wt merge). The model command comes from
-- [commit.generation] command in ~/.config/worktrunk/config.toml (override: SMCLI_COMMIT_CMD).
local usage = [[
usage: lua scripts/commit.lua [options] [--] [pathspec...]

Stages nothing unless asked; the message describes, and the commit holds, what is staged.
  pathspec...        git add these first (git pathspecs: files, dirs, ':!exclude', globs)
  -p, --patch        git add -p (pick hunks) for the pathspecs, or for everything
  -A, --all          git add -A first
  -t, --type TYPE    force the type (feat, fix, ...)
  -s, --scope SCOPE  force the scope
  -m, --hint TEXT    the why, passed to the model (repeatable)
  --amend            rewrite HEAD: message covers HEAD plus what is staged
  -n, --dry-run      print the message, commit nothing
  -y, --yes          commit without asking
  -e, --edit         open the message in $EDITOR before committing
  --no-verify        pass through to git commit
Interactive keys: y commit, e edit, r regenerate, n abort.]]

local function sh(cmd)
  local f = assert(io.popen(cmd, "r"))
  local out = f:read("a")
  local ok, _, code = f:close()
  return out, ok and 0 or code
end
local function q(s) return "'" .. s:gsub("'", [['\'']]) .. "'" end
local function run(cmd) if not os.execute(cmd) then os.exit(1) end end
local function slurp(path)
  local f = io.open(path, "r"); if not f then return nil end
  local s = f:read("a"); f:close(); return s
end
local function die(msg) io.stderr:write("commit.lua: " .. msg .. "\n"); os.exit(1) end

local o, paths, i = { hints = {} }, {}, 1
while i <= #arg do
  local a = arg[i]
  local function val() i = i + 1; return arg[i] or die(a .. " needs a value") end
  if a == "-h" or a == "--help" then print(usage); os.exit(0)
  elseif a == "-p" or a == "--patch" then o.patch = true
  elseif a == "-A" or a == "--all" then o.all = true
  elseif a == "-t" or a == "--type" then o.type = val()
  elseif a == "-s" or a == "--scope" then o.scope = val()
  elseif a == "-m" or a == "--hint" then table.insert(o.hints, val())
  elseif a == "--amend" then o.amend = true
  elseif a == "-n" or a == "--dry-run" then o.dry = true
  elseif a == "-y" or a == "--yes" then o.yes = true
  elseif a == "-e" or a == "--edit" then o.edit = true
  elseif a == "--no-verify" then o.noverify = true
  elseif a == "--" then table.move(arg, i + 1, #arg, #paths + 1, paths); break
  elseif a:sub(1, 1) == "-" then die("unknown option " .. a .. "\n" .. usage)
  else table.insert(paths, a) end
  i = i + 1
end

local root = sh("git rev-parse --show-toplevel"):gsub("%s+$", "")
if root == "" then die("not in a git repository") end
local spec = ""
for _, p in ipairs(paths) do spec = spec .. " " .. q(p) end

if o.all then run("git add -A") end
if o.patch then run("git add -p --" .. spec)
elseif #paths > 0 then run("git add --" .. spec) end

local base = o.amend and "HEAD~1" or ""
local stat = sh("git diff --staged --stat " .. base)
local diff = sh("git diff --staged --no-color " .. base)
if diff == "" then die("nothing staged") end
local LIMIT = 150000 -- shortcut: large diffs are cut, the stat still lists every file
if #diff > LIMIT then diff = diff:sub(1, LIMIT) .. "\n[diff truncated]\n" end

local TYPES = { feat = 1, fix = 1, refactor = 1, perf = 1, docs = 1, test = 1, chore = 1,
  build = 1, ci = 1, style = 1, revert = 1 }
local RULES = [[
Rules (caveman-commit):
- Subject: <type>(<scope>): <imperative summary>. Scope optional. Types: feat, fix,
  refactor, perf, docs, test, chore, build, ci, style, revert. Add ! after the type or
  scope for a breaking change.
- Imperative mood ("add", not "added"/"adds"). Lowercase after the colon. 50 chars or
  less when possible, hard cap 72, no trailing period.
- Why over what: the diff already says what changed.
- Skip the body when the subject is self-explanatory. Add one only for non-obvious why,
  breaking changes, migrations, security fixes or reverts (always for those). Blank line
  after the subject, wrap at 72, bullets with -, issue refs (Closes #N) at the end,
  BREAKING CHANGE: footer for breaking changes.
- Never write "this commit", "I", "we", "now", "currently", "as requested", emoji, AI
  attribution, or the file name the scope already names.
- Output the raw commit message only: plain text, no code fences, no quotes, no preamble.]]

-- Returns a list of rule breaks the model can fix (empty when the message is fine).
local function check(m)
  local bad, subject = {}, m:match("^[^\n]*")
  local t, rest = subject:match("^(%a+)%b()!?: (.+)$")
  if not t then t, rest = subject:match("^(%a+)!?: (.+)$") end
  if not t or not TYPES[t] then table.insert(bad, "subject must be <type>(<scope>): <summary> with a listed type") end
  if #subject > 72 then table.insert(bad, "subject is " .. #subject .. " chars; hard cap 72") end
  if subject:match("%.$") then table.insert(bad, "subject ends with a period") end
  if rest and rest:match("^%a+ed%s") then table.insert(bad, "summary must be imperative (\"add\", not \"added\")") end
  if m:match("^[^\n]*\n[^\n]") then table.insert(bad, "blank line required after the subject") end
  for line in m:gmatch("[^\n]+") do
    if #line > 72 and not line:match("https?://") then table.insert(bad, "body line over 72 chars"); break end
  end
  local low = " " .. m:lower():gsub("[^%w']", " ") .. " "
  for _, w in ipairs({ "this commit", "i", "we", "now", "currently", "as requested", "generated with" }) do
    if low:find(" " .. w .. " ") then table.insert(bad, "do not write \"" .. w .. "\"") end
  end
  return bad
end
local cmd = os.getenv("SMCLI_COMMIT_CMD")
if not cmd then
  local cfg = slurp((os.getenv("HOME") or "") .. "/.config/worktrunk/config.toml") or ""
  local raw = cfg:match('%[commit%.generation%][^%[]-\ncommand%s*=%s*"(.-)"%s*\n')
  cmd = raw and raw:gsub('\\"', '"'):gsub("\\\\", "\\") or die("no model command (set SMCLI_COMMIT_CMD)")
end

local fix -- rule breaks from the last attempt, fed back once
local function prompt()
  local p = { "Write a git commit message for the staged change below.", RULES,
    "Recent subjects (match their style):", (sh("git log -8 --format=%s")) }
  if o.type or o.scope then
    table.insert(p, ("Use type %s%s."):format(o.type or "of your choice", o.scope and (" and scope " .. o.scope) or ""))
  end
  for _, h in ipairs(o.hints) do table.insert(p, "Why (from the author): " .. h) end
  if fix then table.insert(p, "Your last message broke these rules; fix them:\n- " .. table.concat(fix, "\n- ") .. "\nLast message:\n" .. fix.msg) end
  table.insert(p, "Stat:\n" .. stat .. "\nDiff:\n" .. diff)
  return table.concat(p, "\n\n")
end

local FENCE = string.rep(string.char(96), 3)
local generate
function generate()
  local pf = os.tmpname()
  local f = assert(io.open(pf, "w")); f:write(prompt()); f:close()
  local out, code = sh(cmd .. " < " .. q(pf)); os.remove(pf)
  if code ~= 0 then die("model command failed (" .. tostring(code) .. "):\n" .. out) end
  out = out:gsub("^%s+", ""):gsub("%s+$", "")
  if out:sub(1, 3) == FENCE then out = out:gsub("^[^\n]*\n", ""):gsub("\n?" .. FENCE .. "$", "") end
  if out == "" then die("model returned nothing") end
  out = out .. "\n"
  local bad = check(out)
  if #bad > 0 and not fix then
    fix = bad; fix.msg = out
    out = generate()
  elseif #bad > 0 then
    io.stderr:write("warning: message still breaks rules:\n- " .. table.concat(bad, "\n- ") .. "\n")
  end
  fix = nil
  return out
end

local msg = generate()
local mf = os.tmpname()
while true do
  local f = assert(io.open(mf, "w")); f:write(msg); f:close()
  print(msg)
  if o.dry then os.remove(mf); os.exit(0) end
  local key = (o.yes or o.edit) and "y" or nil
  if not key then
    io.write("[y]es [e]dit [r]egenerate [n]o: "); io.flush()
    key = (io.read("l") or "n"):sub(1, 1):lower()
  end
  if key == "r" then msg = generate()
  elseif key == "y" or key == "e" then
    local c = "git commit -F " .. q(mf) .. ((o.edit or key == "e") and " -e" or "")
      .. (o.amend and " --amend" or "") .. (o.noverify and " --no-verify" or "")
    local ok = os.execute(c); os.remove(mf)
    if ok then os.execute("git status --short") end
    os.exit(ok and 0 or 1)
  else os.remove(mf); print("aborted; staging kept"); os.exit(1) end
end
