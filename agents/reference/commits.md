# Stage, commit, and push

Agent runbook for staging changes, writing commit messages, and pushing. Follow this when the user asks to commit or push. Do not invent commits.

## Authority

- Commit, push, or open a PR only when the user asks for that exact action.
- Dirty worktrees are user-owned: never overwrite, discard, reset, checkout over, or revert user changes unless asked.
- Reset, checkout, force-push, amend, rebase, and tag creation need explicit user intent.
- Prefer free/cheap models for commit-message drafting when delegating (`pigrok45`).

## Inspect first

Run these before staging:

``` sh
git status
git diff
git diff --staged
git log -5 --oneline
```

Use the recent log to match this repo's capitalization and scope habits after the colon.

## Stage

- Stage only files that belong in the commit. Prefer path-specific adds:

``` sh
git add path/to/file1 path/to/file2
```

- Avoid `git add .` / `git add -A` unless the user asked to include everything and the status is clean of junk.
- Never stage secrets or local junk: `.env`, credentials, private keys, large binaries, personal notes, generated noise you did not mean to ship.
- If unrelated dirty files exist, leave them unstaged or ask before touching them.
- Re-check `git status` and `git diff --staged` after staging.

## Commit message (caveman-commit)

Write Conventional Commits messages that are terse and exact. Why over what. The diff already says what changed.

### Subject

``` text
<type>(<scope>): <imperative summary>
```

- `<scope>` optional
- Types: `feat`, `fix`, `refactor`, `perf`, `docs`, `test`, `chore`, `build`, `ci`, `style`, `revert`
- Imperative mood: "add", "fix", "remove" — not "added", "adds", "adding"
- ≤50 chars when possible, hard cap 72
- No trailing period
- Match this repo's capitalization after the colon

### Body

Skip the body when the subject is self-explanatory.

Add a body only for non-obvious *why*, breaking changes, migration notes, or linked issues:

- Wrap at 72 chars
- Bullets use `-` not `*`
- Put issue/PR refs at the end: `Closes #42`, `Refs #17`

Always include a body for breaking changes, security fixes, data migrations, and reverts.

### Never put in the message

- "This commit does X", "I", "we", "now", "currently"
- "As requested by..." — use a `Co-authored-by` trailer
- AI attribution / "Generated with ..." unless the user's own rule requires an `Assisted-by` trailer
- Emoji unless this repo already uses it
- Restating a file name when scope already says it

### Examples

``` text
feat(api): add GET /users/:id/profile

Mobile client needs profile data without the full user payload
to reduce LTE bandwidth on cold-launch screens.

Closes #128
```

``` text
feat(api)!: rename /v1/orders to /v1/checkout

BREAKING CHANGE: clients on /v1/orders must migrate to /v1/checkout
before 2026-06-01. Old route returns 410 after that date.
```

## Commit

Use a HEREDOC so formatting stays exact:

``` sh
git commit -m "$(cat <<'EOF'
type(scope): imperative summary

Optional body explaining why.

EOF
)"
```

- Do not amend unless the user explicitly asks and the amend target is safe (for example, the commit was created by you in this session, not yet pushed, and hooks are not being skipped casually).
- Do not use `--no-verify` unless the user explicitly asks to skip hooks.
- After committing, show `git status` so the user can see the result.

## Push

Push only when asked.

``` sh
git push
```

For a new branch that has no upstream yet:

``` sh
git push -u origin HEAD
```

Rules:

- Never force-push to `main` / `master`.
- Never `--force` / `--force-with-lease` unless the user explicitly asks for that exact action.
- Do not create tags unless asked.
- If push is rejected, report the remote error and stop; do not rewrite history to force it through.

## Checklist

1. User asked to commit and/or push.
2. Status/diff/log inspected.
3. Only intended paths staged; no secrets.
4. Caveman-commit subject (and body if required) ready.
5. Commit via HEREDOC.
6. Push only if requested, without forced history rewrites.
