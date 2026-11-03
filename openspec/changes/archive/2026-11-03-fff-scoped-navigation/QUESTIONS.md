# Implementation notes / questions

## `cwd` field added to the files/grep wire request

The design doc says "requests carry [scope] as `directory`; a new `scope: true` flag tells the service to use it verbatim as the fff root." The spec, however, requires inserted paths to be relative to the cwd when possible, and `~/…` or absolute otherwise.

If `directory` becomes the scope, the service no longer receives the cwd and cannot compute cwd-relative inserts. To satisfy both the scope flag and the insert requirement, the implementation keeps `directory` as the scope and adds a separate optional `cwd` field that the caller populates with the original cwd. The service then uses `cwd` for `relative_insert` while using `directory` as the fff root when `scope: true`.

This is a small wire addition beyond the listed `scope`/`path` fields. If the intended design was different, the `cwd` field can be removed and `directory` reverted to mean cwd, with the scope directory carried in a separate string field instead.
