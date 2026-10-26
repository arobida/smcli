# Installing smcli

smcli is two executables that must stay side by side:

- `smcli`: the command you run, and the target of the shell hooks.
- `smcli-service`: a background helper that `smcli` starts on demand and that exits when idle. It keeps history in
  memory and powers the file and grep picker. `smcli` finds it in its own directory (`smcli` → `smcli-service`,
  `smcli-<build>` → `smcli-service-<build>`); set `SMCLI_SERVICE_BIN` to an absolute path to override.

Both depend only on system libraries; the fff file-search library is built in. The Potion search model is shipped separately in the `potion-shell-16m/` folder beside both executables.

Requirements: macOS 14 or later, or Linux. Bash 4+ for the picker keys (macOS's `/bin/bash` 3.2 still records history, without
keys), any recent Zsh, or Fish.

- [Quick install with the script](#quick-install-with-the-script) (recommended)
- [Install a release](#install-a-release)
- [Install from source](#install-from-source)
- [Set up your shell](#set-up-your-shell) (both)
- [Upgrade](#upgrade) · [Uninstall](#uninstall) · [Troubleshooting](#troubleshooting)

## Quick install with the script

`install.sh` does everything below in one step:

- It installs both executables and `smcli-uninstall` to `~/.local/bin`, clearing the macOS quarantine flag.
- It adds the shell hooks to your startup files.
- It asks for the picker keys and search model.
- It starts the service.

``` sh
./install.sh                 # from an unpacked release (the binaries sit beside it)
scripts/install.sh           # from a source checkout: builds with `make release` first (needs Rust)
```

| Option          | Meaning                                                                                           |
| --------------- | ------------------------------------------------------------------------------------------------- |
| `--prefix DIR`  | Where the executables go (default `~/.local/bin`)                                                 |
| `--from DIR`    | Install the `smcli` and `smcli-service` found in `DIR` (e.g. `dist`)                              |
| `--shells LIST` | `zsh,bash,fish` to hook; `all` (every one installed) or `none`. Default: your login shell         |
| `--keys LIST`   | Picker keys: `ctrl-r,left,right`, `ctrl-r`, `left`, `right` or `none`. Default: ask in a terminal |
| `--yes`         | Ask nothing; without `--keys`, keeps the default keys (Up and Ctrl-R)                             |

The hooks go in a marked block at the end of each file, so they load after other tools that bind keys:

``` sh
# >>> smcli >>>
# Added by smcli install.sh; remove with smcli-uninstall. Keep it below other tools that bind keys.
eval "$('/Users/you/.local/bin/smcli' shell-init zsh)"
# <<< smcli <<<
```

Zsh uses `$ZDOTDIR/.zshrc` (else `~/.zshrc`), Bash `~/.bashrc`, and Fish `$XDG_CONFIG_HOME/fish/config.fish`. Each
file is backed up first as `<file>.smcli-backup`. Rerunning the script upgrades in place: it replaces the binaries
and the block, and never adds a second block. For Bash, it warns when `~/.bash_profile` doesn't read `~/.bashrc`
(macOS opens login shells).

Unattended, e.g. in a dotfiles bootstrap: `scripts/install.sh --shells all --keys ctrl-r --yes`.

Open a new terminal afterwards. The rest of this page is the same steps done by hand.

## Install a release

1. Unpack the release archive and copy both executables to one directory on your `PATH`:

   ``` sh
   mkdir -p ~/.local/bin
   cp smcli smcli-service ~/.local/bin/
      cp -R potion-shell-16m ~/.local/bin/
   ```

   Keep them together. Moving only `smcli` leaves it without its service: commands still work, but more slowly,
   and the file and grep picker stops working.

2. If macOS blocks a downloaded binary ("cannot be opened because the developer cannot be verified"), clear the
   quarantine flag:

   ``` sh
   xattr -d com.apple.quarantine ~/.local/bin/smcli ~/.local/bin/smcli-service
   ```

3. Continue with [Set up your shell](#set-up-your-shell).

## Install from source

Tools:

- A C toolchain: Xcode or the Command Line Tools on macOS; `build-essential` on Debian/Ubuntu.
- Rust via [rustup](https://rustup.rs) (`cargo`). `fff-search` and its vendored libgit2 are built from source:
  the first build downloads and compiles the locked crate sources; later builds reuse them.
- On Linux, `pkg-config` and the SQLite development headers (e.g. `libsqlite3-dev` on Debian/Ubuntu).
- `curl` and `git` (included with macOS and most Linux distributions).

Build:

``` sh
git clone <repository> smcli && cd smcli
make fresh
```

`make fresh` does a clean release build into `dist/`: `dist/smcli-<timestamp>-<commit>` and
`dist/smcli-service-<timestamp>-<commit>`, plus `dist/smcli` and `dist/smcli-service` symlinks to the newest pair.
`make release` does the same without cleaning first (faster). `make clean` keeps the newest pair and both
symlinks, because your shell hooks may point at them.

Point your shell at the `dist/smcli` symlink, not at a timestamped file, so every rebuild is picked up by new
shells:

``` sh
# in ~/.zshrc (use the absolute path of your checkout)
eval "$("$HOME/Dev/smcli/dist/smcli" shell-init zsh)"
```

To try a build in the current shell only, run the `eval` line that `make fresh` prints.

For development:

| Command                      | What it does                                                                                    |
| ---------------------------- | ----------------------------------------------------------------------------------------------- |
| `make build`                 | Debug build (`target/debug/`)                                                                   |
| `make run ARGS="search git"` | Debug build, then run it                                                                        |
| `make test`                  | Unit tests, then the interactive Bash/Zsh/Fish, service and file-picker tests on real terminals |
| `make ranking`               | Ranking quality report; fails when the ranking tests fail                                       |
| `make baseline`              | Black-box benchmark of the release build                                                        |

`make test` uses throwaway homes and databases and never reads your real history. Shells that aren't installed are
skipped and reported. On Linux, install the packages above, then run `make release`. `make linux` runs the full
suite in a `rust:latest` Docker container (needs a running Docker daemon).

Then continue with [Set up your shell](#set-up-your-shell).

## Set up your shell

1. **Choose the picker keys and the search model:**

   ``` sh
   smcli init
   ```

   It asks which picker keys to bind (press Enter to keep the default):

   ```
   Open history with Ctrl-R? [Y/n]
   Open history with Left on an empty line? [y/N]
   Open suggestions with Right on an empty line? [y/N]
   Open files with Ctrl-T? [Y/n]
   Open grep with Ctrl-G? [Y/n]
   ```

   Up always opens the picker. On an empty line in Bash and Zsh, Left opens the same history search as Ctrl-R and Right opens directory suggestions. A line that already has text keeps normal arrow movement. Then it offers any Ollama embedding models you have installed; the default is the
   potion-shell-16m model, installed in the `potion-shell-16m/` folder beside the executables.

   For scripts and dotfiles, use flags instead; they never ask:

   ``` sh
   smcli init --keys ctrl-r,left,right     # or ctrl-r, left, right, none
   smcli init --default --keys ctrl-r   # model and keys together
   ```

   Settings are saved to `~/.config/smcli/config.json` (or `$XDG_CONFIG_HOME/smcli/config.json`, or `SMCLI_CONFIG`).

2. **Load the hooks.** Add one line to your shell's startup file. Use the binary's absolute path:

   | Shell | File                         | Line                                           |
   | ----- | ---------------------------- | ---------------------------------------------- |
   | Zsh   | `~/.zshrc`                   | `eval "$(~/.local/bin/smcli shell-init zsh)"`  |
   | Bash  | `~/.bashrc`                  | `eval "$(~/.local/bin/smcli shell-init bash)"` |
   | Fish  | `~/.config/fish/config.fish` | `~/.local/bin/smcli shell-init fish \| source` |

   Put the line **after** other tools that bind keys (fzf, atuin, oh-my-zsh, zsh-autosuggestions): smcli leaves
   keys that another tool already bound alone and says which it kept. The hooks never modify your native history
   file.

   Optional, set before the line:

   - `SMCLI_NO_KEYS=1`: bind no keys (history is still recorded).
   - `SMCLI_EXTRA_KEYS=1`: also Left and Right on an empty line, Ctrl-Right and Alt-; (Bash and Zsh). Fish keeps Right for autosuggestions.
   - `SMCLI_DB=/path/history.db`: another database (it must not be your shell's `HISTFILE`).

3. **Open a new terminal and check:**

   ``` sh
   smcli health            # search index and model
   smcli service status    # the background service (it starts on first use)
   ```

   Run a few commands, then press Up on an empty prompt. Optionally, bring in existing history:
   `smcli import --atuin`.

Key choices take effect in new shells. To change them later, run `smcli init --keys` (it asks) or pass a list.

## Upgrade

- **Script:** run `install.sh` again (from the new release, or `git pull && scripts/install.sh` in a checkout).
- **By hand, release:** replace both executables together.
- **By hand, source:** `git pull && make fresh`.

A running service from the previous build is stopped and replaced automatically on the next command. Open a new
terminal so the shell reloads the new hooks; until then, inserting file paths from the picker is refused with a
message asking you to reload.

## Uninstall

``` sh
smcli-uninstall
```

It asks once, then:

1. **Exports your history** to `~/smcli-export-<date>/` (`--export DIR` to choose):

   - `history.db` is a consistent copy of the database. Use it again with `SMCLI_DB=…/history.db`, or open it with
     `sqlite3`.
   - `history.json` holds every recorded command with its directory, exit code, times and session.
   - `config.json` is your settings.

   Export requires `sqlite3`; install it or pass `--no-export` to skip the export. The export directory is private
   (`0700`), and exported history and config files are owner-readable/writable only (`0600`), including existing
   destinations. If the export fails, nothing is removed.

2. Stops the service.

3. Removes the smcli block from each startup file and leaves the rest of the file as it was. It warns about
   `shell-init` lines you added by hand, which it doesn't touch.

4. Deletes the history database, search index, settings and service socket.

5. Deletes `smcli`, `smcli-service` and itself.

| Option         | Meaning                                                             |
| -------------- | ------------------------------------------------------------------- |
| `--export DIR` | Where the export goes                                               |
| `--no-export`  | Delete the history without exporting it                             |
| `--keep-data`  | Remove the program and hooks only; keep history, index and settings |
| `--prefix DIR` | Where smcli is installed (default: the uninstaller's own directory) |
| `--yes`        | Don't ask (required when not run from a terminal)                   |

Terminals that are already open keep the old hooks until they exit. Native Bash, Zsh and Fish history is never
touched.

By hand: run `smcli service stop`, remove the `shell-init` line, delete the executables, then delete these:

| What             | Where                                                                           |
| ---------------- | ------------------------------------------------------------------------------- |
| Recorded history | `~/.local/share/smcli/history.db` (or `SMCLI_DB`)                               |
| Search index     | `~/.local/share/smcli/history.db.search/`                                       |
| Settings         | `~/.config/smcli/`                                                              |
| Service socket   | `$SMCLI_RUNTIME_DIR`, else `$XDG_RUNTIME_DIR/smcli`, else `$TMPDIR/smcli-<uid>` |

## Troubleshooting

- **No picker on Up or Ctrl-R:** another tool may own the key; the shell prints `smcli: kept existing key bindings: …` at startup. Move the smcli line after that tool, or bind the key to `_smcli_pick` yourself. Also
  check `SMCLI_NO_KEYS` isn't set and, for Bash, that it's 4.0 or later (`echo $BASH_VERSION`).
- **"files and grep need smcli-service":** `smcli-service` isn't next to `smcli`, or `SMCLI_SERVICE=off` is set.
- **"not inserting a path inside an open quote":** close the quote first. File paths are only inserted where the
  shell reads plain words.
- **`smcli: command metadata was not saved`:** run `SMCLI_SYNC=1` in the shell to record synchronously and see the
  error, or check that the `SMCLI_DB` directory is writable.
- **Alt-; does nothing (macOS):** set your terminal's Option key to send Meta/Esc+.
