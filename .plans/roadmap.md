# Roadmap

Future work, not scheduled. macOS (arm64 and x86_64) is the only supported platform today.

## Linux support

smcli doesn't build on Linux today. Known work:

- **Directory context:** `ContextCache` invalidates on FSEvents. Linux needs inotify (or fff's watcher) instead.
- **fff:** add a Linux target to `scripts/build-fff.sh` and the Makefile (x86_64 and aarch64 `libfff_c.a`), and check
  the libgit2/OpenSSL linking.
- **Service:** the socket path, spawn and permissions in `UnixSocket.swift` and `ServiceClient.swift`. Check
  `XDG_RUNTIME_DIR` instead of the macOS temp directory.
- **Terminal:** the picker's `/dev/tty` read-timeout workaround is macOS-specific. Use `poll()` on Linux.
- **Search:** confirm ProximaKit and the embedded Potion model build and run under swift-corelibs-foundation.
- **Tests:** run `make test` (Swift, shell and service tests) on Linux in CI, for bash, zsh and fish.
- **Packaging:** decide on tarball, .deb or Homebrew-on-Linux, and apply `shipit-checklist.md` to those artifacts.

## Other follow-ups from the backend plan

- fgrep on large trees (see `fgrep-notes.md`).
- Make the search corpus incremental for writes from other processes (today they force a full rebuild of about 760 ms).
- Fish: sub-second start times, and read fish's native history file.
- Record bash pipestatus.
- Run the Swift unit and shell tests on x86_64.
