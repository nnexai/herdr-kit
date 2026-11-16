# Distributing coordinator work across machines — plan

Plan, 2026-11-16. Follows [multi-machine-research.md](multi-machine-research.md): **one authoritative coordinator, other machines run delegates through Herdr machines (`hc run --on`).** Nothing here is implemented or tested; remote behaviour of `hc` is taken from installed `hc 0.1.21` help and is [INFERENCE] until the probes below have run.

A refinement of the research: installed `hc` already provides `run --on`, `exec --on`, `file read|write --on` and `machine add|update` over SSH. Through M3 these cover transport, so **no owner API is needed**. That API is only needed later, for clients outside Herdr such as Telegram or a phone.

## 1. What runs where

```mermaid
flowchart LR
  subgraph D["Desktop: authority"]
    DA[herdr-app + Board/Mission Log]
    DC[herdr-core serve]
    CO[kit-coord coordinator + watcher]
    DJ[(coord/ journal, briefs, results)]
    DD[local delegates: omp]
  end
  subgraph B["Always-on box: worker"]
    BC[herdr-core serve, systemd + linger]
    BD[delegates: omp]
    BO[(worker outbox + sessions)]
    BM[optional model server]
  end
  subgraph W["Extra worker(s)"]
    WC[herdr-core serve]
    WD[delegates: omp]
  end
  CO -- "hc run/exec/file --on (SSH over Tailscale)" --> BC & WC
  CO -- "pull: kit-coord worker drain" --> BO
  BD -. model API on tailnet .-> BM
```

| Machine | Processes | Must be up for | When it is off |
|---|---|---|---|
| Desktop (authority) | herdr-core, herdr-app, coordinator + watcher, local delegates, the person's UI | new placements, answers, the inbox and cleanup | Nothing is decided. Remote delegates keep working and queue their reports in the worker outbox. On boot, `kit up` autostarts, then the watcher reconciles and drains the outboxes. |
| Always-on box (worker) | herdr-core only (no app), delegates, worker outbox, optional `llama-server`/Ollama on the tailnet | tasks placed on it | Its tasks show `host_unreachable` and are **retained, never released**. The coordinator places new work elsewhere and reattaches or resumes on return. |
| Extra workers | herdr-core, delegates | their own tasks | Same as the box; laptops may sleep freely. |

Repositories, OMP/Git credentials and session transcripts stay on each host. Only briefs (sent out) and result files (pulled in) cross hosts. The model server is optional: omp on any host can point at it. That is the person's own omp config, which herdr-kit does not edit; a user-defined role in `coord/roles/` can select it. Moving the authority to the box is M4 and optional.

## 2. Distributing work

**Placement.** Add one orthogonal flag, `kit task start --on <host>`, recorded on the workstream as its host and inherited by follow-ups:

- Rung 1 is pinned to the host of its session.
- Rungs 2 and 3 inherit the host of the space.
- Rungs 4 and 5 take `--on`; the default is the authority.

Picking a host requires a recorded reason, the same way the ladder does (`--reject-host local="repo not cloned"`). Hosts are entries in `coord/hosts.json` on the authority:

```json
{"box": {"machine": "box", "repos": {"herdr-kit": "/path/to/herdr-kit"},
         "worktree_root": "~/.herdr/worktrees", "roles": ["coord-default","coord-light"],
         "max_delegates": 3, "providers": ["anthropic"], "epoch": 1}}
```

The coordinator chooses the host with three tests:

1. **Repo locality:** the logical repo must be cloned there.
2. **Role and credentials:** the role must be allowed and its provider logged in.
3. **Capacity:** live delegates must be below `max_delegates`. Quota is per host because each host has its own omp login; `kit hosts` can show `hc exec --on <m> omp usage`-style readouts [INFERENCE].

If nothing fits, it asks the person; it never guesses.

**Briefs and results.**

- **Brief:** written locally as today, then copied with `hc file write --on <m> <worker-data>/coord/briefs/<task>.md`.
- **Launch:** `hc run --on <m> --cwd <host path> --label coord.task=<id> --label coord.epoch=<n> -- kit-coord worker exec --epoch <n> -- omp …`.
- **Report:** the delegate runs the same `kit-coord report` command. On a worker data dir (marked by `coord/worker.json`), the report is appended to a local **outbox** together with its terminal ID and a request ID.
- **Collection:** the authority's watcher pulls with `hc exec --on <m> kit-coord worker drain --after <ack>`. It first stores the result contents with `hc file read --on`, then journals `task.reported` and acknowledges.

Pulling means workers never need to reach the authority, reports survive network loss, and request IDs make a redelivered report a no-op.

**Remote release.** Release and reconcile run their Git and terminal checks through a host executor: local `Command`, or `hc exec --on <m> git -C …`. The existing checks are unchanged: dirty, reachable, merged, and process identity. Delegates push branches to the shared Git remote, so reachability is checked after a fetch on the worker. An unreachable host gives the retained reason `host_unreachable`.

**What kit-coord lacks today**

| Gap | Where |
|---|---|
| `RunSpec` has no machine, so `hc run` never passes `--on` | [adapter.rs#L12](../../crates/kit-coord/src/coord/adapter.rs#L12), [#L193](../../crates/kit-coord/src/coord/adapter.rs#L193) |
| Placement targets carry no host; rung 5 only checks for an absolute local path | [place.rs#L183](../../crates/kit-coord/src/coord/place.rs#L183) |
| Spaces, tasks and resources store bare paths and terminal IDs with no host | [state.rs#L104](../../crates/kit-coord/src/coord/state.rs#L104), [#L119](../../crates/kit-coord/src/coord/state.rs#L119), [#L187](../../crates/kit-coord/src/coord/state.rs#L187) |
| A report canonicalizes a **local** result file and resolves its caller from a local `HC_TERMINAL_ID` | [place.rs#L1100](../../crates/kit-coord/src/coord/place.rs#L1100), [caller.rs#L32](../../crates/kit-coord/src/coord/caller.rs#L32) |
| The brief footer embeds the authority's absolute CLI and result paths | [brief.rs#L8](../../crates/kit-coord/src/coord/brief.rs#L8), [place.rs#L569](../../crates/kit-coord/src/coord/place.rs#L569) |
| Session discovery scans a local directory | [launch.rs#L116](../../crates/kit-coord/src/coord/launch.rs#L116) |
| All Git operations are local `git -C` | [git.rs](../../crates/kit-coord/src/coord/git.rs), [release.rs#L138](../../crates/kit-coord/src/coord/release.rs#L138) |
| **Unreachable is treated as gone:** `hc.get(..) == None` releases a tab | [reconcile.rs#L340-L361](../../crates/kit-coord/src/coord/reconcile.rs#L340) |
| An event-stream outage is global, not per host | [watch.rs#L442](../../crates/kit-coord/src/coord/watch.rs#L442) |

## 3. Restartable

| Component | Crash / reboot | Network loss |
|---|---|---|
| herdr-core | Already runs as a `systemd-run` unit with `keeps_running_after_logout` (observed: `hc server status`). On workers, add a persistent user unit, `herdr-core.service` with `Restart=always`, and enable linger so it survives reboot. Programs in kept terminals die on reboot; the terminals come back as shells. | Terminals keep running locally. |
| Watcher | Single instance via the `watcher.lock` flock ([watch.rs#L465](../../crates/kit-coord/src/coord/watch.rs#L465)). Heartbeat every 5 s. `up` replaces a stale watcher ([lifecycle.rs#L173](../../crates/kit-coord/src/coord/lifecycle.rs#L173)). | Probes each host with `hc exec --on m true` and records `host.unreachable`/`host.reachable` events, with backoff. |
| Coordinator | `up --if-enabled` from the activation hook ([activation.rs#L32](../../crates/kit-manager/src/manager/activation.rs#L32)), serialized by `up.lock` ([lifecycle.rs#L61](../../crates/kit-coord/src/coord/lifecycle.rs#L61)). A fresh session reads the board and the handoff note from the journal. If the authority later lives on the box, a person-installed oneshot `kit-coord-up.service` runs the same idempotent `up --if-enabled` after boot. | Not applicable on the authority itself. |
| Delegates | The watcher marks `task.interrupted` when a host is reachable but the labelled terminal is absent. The coordinator resumes on the same host with `--session-of`, using the role's `resume` command ([role.rs#L22](../../crates/kit-coord/src/coord/role.rs#L22)); session files stay on that host. | The terminal still runs and reports queue in the outbox. **Unknown ≠ absent:** host presence becomes three-state (present / absent / unknown), and release refuses on unknown. |

- **Idempotent launch:** the existing intent-before-action record ([state.rs#L197](../../crates/kit-coord/src/coord/state.rs#L197), settled in [reconcile.rs#L91](../../crates/kit-coord/src/coord/reconcile.rs#L91)) gains host and epoch labels. An uncertain `hc run --on` is resolved by `hc ls --label coord.intent=<seq>` on that machine before any retry.
- **No duplicate authority:** on one host, the flocks already exclude duplicates. Across hosts, the `coord/` tree is never synced, and an **authority epoch** is added: every launch and drain carries it, and `kit-coord worker exec|drain` refuses anything below the highest epoch that worker has seen. A desktop that comes back after the authority moved to the box (M4) is therefore fenced out by the workers themselves, without trusting heartbeats. Coordinator lifecycle stays person-only.

## 4. Quick to set up (target: under 10 minutes)

The person does these steps by hand, because they are logins:

1. On the new machine, install Tailscale and run `tailscale up` (browser login). Enable Tailscale SSH, or add the authority's SSH key.
2. On it, run `omp` once and log in (browser or device code). Run `gh auth login`, or add a deploy key, for Git push.
3. Optionally, if `loginctl enable-linger` needs an admin, allow linger for the user.

Then one command on the authority:

```sh
herdr-kit worker add box --ssh agent@box --repo herdr-kit=git@github.com:nnexai/herdr-kit.git
```

It runs these steps idempotently and can be re-run:

1. Checks that SSH works.
2. `hc machine add box --install`.
3. Copies the active release's `kit-coord` with `hc file write --on`, marked as a worker by `worker.json`.
4. Installs and enables `herdr-core.service` and linger.
5. Runs `git clone` (or `fetch`) into the target path.
6. Checks `omp --version` and auth, reporting any missing logins.
7. Writes the host record with epoch 1.
8. Runs a smoke test: `hc run --on box -- kit-coord worker exec --dry-run`.

`hc machine update box` keeps `hc` current. `herdr-kit update apply` re-pushes the worker binary.

## 5. Milestones

| # | Usable result | Size | Probe first | Riskiest unknown |
|---|---|---|---|---|
| M1 | **Remote delegate.** `--on` for rungs 3 and 4 on a manually added host, with remote brief, outbox report and result pull. Remote release is retained as `remote_release_manual`, and the person cleans up. | M, ~3–4 d | P1: `hc machine add box --install`, then `hc run --on box --bg --keep --name probe --label coord.task=probe --cwd ~/dev/herdr-kit -- omp --model @smol "reply hi"`. Check `hc get box/tN`, `hc events` working/idle words, `hc send` steering, `HC_TERMINAL_ID` inside, and whether `herdr-app ls` shows the remote space. | Whether labels, `--keep` and the app space/tab view work for remote terminals. |
| M2 | **Host-aware reconcile and release.** Three-state presence, `host_unreachable` retention, remote Git checks, resume on host, epoch-tagged launches. | M, ~1 wk | P2: during P1, run `tailscale down` on the box, then record what `hc get box/tN`, `hc ls --label` and `hc events` return, compared with a truly killed terminal. | Whether "unreachable" and "absent" can be told apart at all. Until they can, [reconcile.rs#L353](../../crates/kit-coord/src/coord/reconcile.rs#L353) must skip remote resources. |
| M3 | **`herdr-kit worker add`** and worker systemd units; a new machine in under 10 minutes. | S, ~2–3 d | P3: bootstrap a fresh VM by hand with a stopwatch, and test headless omp login. | omp login without a browser, and linger permissions. |
| M4 (optional) | **Authority on the always-on box.** `kit authority move` with an epoch bump; desktop Board/Mission Log run through `hc exec --on box kit-coord …`. | L, ~1–2 wk | P4: run herdr-app, or only `herdr-core`, on the box, and run `kit-coord status`/`board` there through `hc exec --on` from the desktop mods. | kit-coord's dependency on herdr-app for spaces ([adapter.rs#L133](../../crates/kit-coord/src/coord/adapter.rs#L133)) on a headless host. |

Order: run P1 and P2 before writing any code. If P2 shows that unreachable and absent cannot be distinguished, M1 ships with remote resources excluded from automatic release.
