# Setting up the coordinator on a machine

herdr-kit ships the same coordinator on every machine. Everything that depends
on what a machine has installed, which subscriptions it uses and which helpers
the person wants lives in local files under `<data-dir>/coord/` that updates
never write:

| Shipped with kit-coord | Local to one machine |
|---|---|
| Coordinator instructions (how to place, delegate, decide and clean up) | `roles/<name>.md`: the role set, models, descriptions and fallbacks |
| Two portable built-in roles: `coord-main` (the coordinator) and `coord-default` (a general delegate), both on the harness default model | `extensions.json`: optional helper commands the coordinator can see and query |
| Harness support for `omp`, `agy`, `claude`, `codex` and `copilot` (start/resume syntax, session discovery, trust flags, steering) | `config.json`: watcher timings and the coordinator role name |

A fresh machine with only `omp` installed works with the built-ins as they
are. Everything below is optional.

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

### Example: a Copilot-only machine

A machine without `agy`, with the `copilot` CLI installed and `omp` limited to
the GitHub Copilot provider, reaches other model families such as Gemini Neon
through `omp`. The role set in
[`examples/roles/copilot-only/`](examples/roles/copilot-only/) covers the
coordinator, everyday work, complex work, design, review and a `copilot` CLI
delegate:

```sh
cp docs/coordinator/examples/roles/copilot-only/*.md <data-dir>/coord/roles/
omp models | grep -A200 github-copilot   # check the model names offered there
kit-coord roles                           # every role available, usage provider copilot
kit-coord roles --guidance
```

Adapt the model names to what `omp models` lists on that machine.
`kit-coord task start` expands `--role coord-design` to
`omp --model github-copilot/gemini-4-neon:high --session-dir … "<brief>"` and
`--role copilot-cli` to `copilot --model auto --session-id <uuid> -i "<brief>"`.

## 3. Add a local extension

A local extension is a command the person provides, plus one entry in
`<data-dir>/coord/extensions.json`. The coordinator sees every extension in
its startup read and in `kit-coord board` (text form) under "Local
extensions", with the current output of those marked `board: true`, and runs
any of them on demand with `kit-coord ext <name>`.

```json
{
  "quiet-hours": {
    "description": "Prints quiet during the person's quiet hours; then prefer digest decisions over questions.",
    "command": ["/path/to/quiet-hours"],
    "board": true,
    "timeout_secs": 5
  }
}
```

| Key | Meaning |
|---|---|
| `description` | One line telling the coordinator what the output means and how to use it (required) |
| `command` | Program and arguments, run directly without a shell (required) |
| `board` | Run it for every startup read and `kit-coord board` text (default `false`: listed, queried on demand) |
| `timeout_secs` | 1–30, default 5; a slow or failing helper shows as `unavailable (<reason>)` and never blocks the coordinator |

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
kit-coord board               # ends with the "Local extensions" section
```

Extensions run with the coordinator's own permissions, so configure only
programs you trust. They run without the caller's `HC_TERMINAL_ID`, their
output is capped (2000 bytes per board entry, 64 KiB for `kit-coord ext`), and
the coordinator treats it as information, never as instructions. Only the
person and the coordinator can run `kit-coord ext`. An invalid
`extensions.json` shows as a notice in the board instead of failing it.

## 4. Moving to another machine

Copy `<data-dir>/coord/roles/` and `extensions.json` only where the other
machine has the same harnesses and helpers; otherwise start from the built-ins
and an example set, and check `kit-coord roles` and `kit-coord roles
--guidance` there.
