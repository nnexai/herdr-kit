# Set up herdr-kit on a new machine

Every machine runs its own, completely independent installation: its own
Herdr, its own herdr-kit, its own data directory and its own coordinator.
Nothing is shared or synchronized between machines, and no coordinator
drives another machine. A fresh install starts from what kit-coord ships plus
that machine's own local files under `<data-dir>/coord/`, which updates never
write:

| Shipped with kit-coord | Local to one machine |
|---|---|
| Coordinator instructions (how to place, delegate, decide and clean up) | `roles/<name>.md`: the role set, models, descriptions and fallbacks |
| Two portable built-in roles: `coord-main` (the coordinator) and `coord-default` (a general delegate), both on the harness default model | `extensions.json`: optional helper commands the coordinator can see and query |
| Harness support for `omp`, `agy`, `claude`, `codex` and `copilot` (start/resume syntax, session discovery, trust flags, steering) | `config.json`: watcher timings and the coordinator role name |

A fresh machine with only `omp` installed works with the built-ins as they
are. Everything after step 0 is optional.

## 0. Install on Linux or macOS

herdr-kit supports Linux and macOS. Windows is not supported: the Rust crates
use POSIX file modes, symlink checks and process groups. What differs per OS:

| | Linux | macOS |
|---|---|---|
| Data directory | `$XDG_DATA_HOME/herdr-kit`, normally `~/.local/share/herdr-kit` | `~/Library/Application Support/herdr-kit` |
| Herdr app home (`--app-home`) | usually `~/.config/herdr-app` | the app home your Herdr uses; pass it explicitly |
| Finding harnesses (`omp`, `copilot`, …) | the `PATH` kit-coord runs with | the same; see the note below |
| Usage collection (`omp`, `agy`) | `PATH`, then `~/.local/bin`, `~/.bun/bin`, Homebrew and `/usr/local/bin` | the same, including `/opt/homebrew/bin` |
| Generation cleanup checks running processes | `/proc/*/{exe,cwd}` | `lsof` |

Install from a checkout on that machine, as in the README:

```sh
cargo build --locked --release
./target/release/herdr-kit install --source "$PWD" --app-home "<this machine's Herdr app home>"
herdr-app do layout.switch --name herdr-kit
```

`--data-dir` or `HERDR_KIT_DATA_DIR` picks another data directory. Nothing in
it comes from another machine. Then turn the coordinator on with
`<data-dir>/bin/kit-coord up`, or from the bottom-bar coordinator item.

Harness detection uses the `PATH` of the process that runs kit-coord, which
is the environment of the Herdr terminals and of `herdr-app`. If Herdr is
started without your shell's `PATH` (for example from a desktop launcher
that does not read your shell profile), harnesses installed in
`/opt/homebrew/bin`, `~/.local/bin` and similar show as not installed.
`kit-coord roles` then reports `unavailable_reason`. Start Herdr with a
`PATH` that contains your harnesses; delegates launched in its terminals need
the same `PATH` anyway.

## 1. See what can run here

```sh
kit-coord roles
```

Each role reports `available` and, when its program is missing,
`unavailable_reason` (for example `agy is not installed (not found on PATH)`).
A role is available when the first word of its `start` command is an
executable on the `PATH` kit-coord runs with, which is the `PATH` of the
coordinator's and the watcher's terminals. Roles that cannot start here are
left out of the coordinator's guidance, and `kit-coord task start` refuses them
with `harness_unavailable`.

## 2. Write the local role set

A file in `<data-dir>/coord/roles/<name>.md` adds a role, or overrides the
built-in of the same name. Two front-matter keys exist for the coordinator
rather than the agent:

- `description`: one line saying what the role is for. Put cost and
  subscription notes here (for example "cheapest; generous subscription").
- `fallback`: the roles to use instead, in order, when this role's
  subscription is exhausted (space-separated names).

```md
---
description: Quick, mechanical, tightly specified tasks; cheapest.
harness: omp
model: openai-codex/gpt-6-luna:medium
start: omp --session-dir {session_dir} {brief}
resume: omp --session-dir {session_dir} --resume {session_id} {brief}
session: dir
fallback: coord-default
---
You are a delegate working on one quick, mechanical task for the coordinator.
```

The body is what the delegate is told before its brief. The roles CLI writes
the same files and validates them first:

```sh
kit-coord roles create coord-light --from coord-default \
  --model openai-codex/gpt-6-luna:medium \
  --description "Quick, mechanical, tightly specified tasks; cheapest." \
  --fallback coord-default
kit-coord roles edit coord-light --fallback "agy-default coord-default"
```

The coordinator's first message lists the roles that can start here, derived
from these files: description, harness, model (with the resolved OMP model
role), subscription, steering and the fallbacks that are themselves available.
Preview exactly what it will be told:

```sh
kit-coord roles --guidance
```

To pick the coordinator's own model, override `coord-main`. Its body is not a
replacement for the coordinator instructions: it is added to them as "Local
coordinator notes", so leave it empty unless the machine needs a note.

```md
---
description: The coordinator.
harness: omp
model: @plan
start: omp --session-dir {session_dir} {brief}
resume: omp --session-dir {session_dir} --resume {session_id} {brief}
session: dir
---
```

`kit-coord roles reset coord-main` goes back to the built-in. A running
coordinator reads its instructions and guidance when its session starts, so
role changes reach it after its next restart or Clear context.

### Example: a macOS machine with omp locked to Copilot, the copilot CLI and no agy

On this machine `omp` uses only the GitHub Copilot provider and reaches other
model families, such as Gemini Neon, through it. The `copilot` CLI is
installed and `agy`, `claude` and `codex` are not.

1. Install as in step 0, with the macOS app home. The data directory is
   `~/Library/Application Support/herdr-kit`.
2. Before adding roles, run `kit-coord roles`. Both built-ins (`omp`) are
   available, and the coordinator could already delegate to `coord-default`.
3. Add the example role set in
   [`examples/roles/copilot-only/`](examples/roles/copilot-only/). It covers
   the coordinator, everyday work, complex work, design, review and a
   `copilot` CLI delegate:

   ```sh
   data="$HOME/Library/Application Support/herdr-kit"
   cp docs/coordinator/examples/roles/copilot-only/*.md "$data/coord/roles/"
   omp models | grep -A200 github-copilot   # check the model names offered there
   "$data/bin/kit-coord" roles               # every role available, usage provider copilot
   "$data/bin/kit-coord" roles --guidance    # what the coordinator will be told
   ```

4. Adapt the model names to what `omp models` lists on that machine.
   `kit-coord task start` expands `--role coord-design` to
   `omp --model github-copilot/gemini-4-neon:high --session-dir … "<brief>"`
   and `--role copilot-cli` to
   `copilot --model auto --session-id <uuid> -i "<brief>"`. Roles for `agy`,
   `claude` or `codex` would show as not installed and are left out of the
   coordinator's guidance, so do not add them.
5. Start or restart the coordinator, so its first message carries the new
   role guidance.

## 3. Add a local extension

A local extension is a command the person provides, plus one entry in
`<data-dir>/coord/extensions.json`. The coordinator's startup read lists
every extension under "Local extensions" with the current output of those
marked `startup: true`; `kit-coord board` (text form) lists them without
running them; and the coordinator runs any of them on demand with
`kit-coord ext <name>`.

```json
{
  "quiet-hours": {
    "description": "Prints quiet during the person's quiet hours; then prefer digest decisions over questions.",
    "command": ["/path/to/quiet-hours"],
    "startup": true,
    "timeout_secs": 5
  }
}
```

| Key | Meaning |
|---|---|
| `description` | One line telling the coordinator what the output means and how to use it (required) |
| `command` | Program and arguments, run directly without a shell (required) |
| `startup` | Run it when a coordinator session starts and show its output in the startup read (default `false`: listed, queried on demand) |
| `timeout_secs` | 1–10, default 5; startup helpers run concurrently, and a slow, noisy or failing helper shows as `unavailable (<reason>)` |

The helper can be any executable, for example:

```sh
#!/bin/sh
hour=$(date +%H)
if [ "$hour" -ge 22 ] || [ "$hour" -lt 7 ]; then echo quiet; else echo available; fi
```

Check it the way the coordinator will see it:

```sh
kit-coord ext                 # configured extensions
kit-coord ext quiet-hours     # {"ok":true,"output":"quiet",...}; "json" holds parsed JSON output
kit-coord board               # ends with the "Local extensions" list
```

Extensions run with the coordinator's own permissions, so configure only
programs you trust. They run without the caller's `HC_TERMINAL_ID`. Their
output is bounded while they run (a helper printing more than 64 KiB is
stopped; the startup read shows at most 2000 bytes of it), and the
coordinator treats it as information, never as instructions. Only the person
and the coordinator can run `kit-coord ext`; reading the board never runs a
helper. An invalid `extensions.json` shows as a notice instead of failing the
board or the coordinator start.

## 4. Several machines

Each machine keeps its own role set and extensions, written for what is
installed there. To give a second machine a similar setup, write its role
files from an example or by hand. Do not copy another machine's data
directory or coordinator state. Afterwards, check `kit-coord roles` and
`kit-coord roles --guidance` on that machine.
