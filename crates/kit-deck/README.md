# kit-deck

Standalone OpenDeck / OpenAction (Stream Deck) Rust plugin and CLI for Herdr.

It brings live **herdr-core** agents, **Agent Inbox / Needs you** items, **Coordinator** status, and **subscription limits** (Codex, Claude, Copilot, Antigravity) onto Stream Deck keys via crisp 144×144 dynamic SVG tiles with zero Python runtime or credential access.

## Build & Install into OpenDeck

```sh
cargo build --locked --release -p kit-deck
./target/release/kit-deck install
```

By default `kit-deck install` writes `dev.herdr.kit.sdPlugin` into:
- Linux: `~/.config/opendeck/plugins` (or `~/.var/app/me.amankhanna.opendeck/config/opendeck/plugins` when Flatpak OpenDeck is present)
- macOS: `~/Library/Application Support/opendeck/plugins`

Options:
- `--plugins-dir <DIR>`: target a custom OpenDeck plugins directory
- `--wrapper`: link to the built binary via a small wrapper script instead of copying it
- `--data-dir <DIR>`: bind a non-default `herdr-kit` data directory in `config.json`

To remove it:

```sh
./target/release/kit-deck uninstall
```

## CLI Inspection & Preview

```sh
# Print live snapshot of agents, coordinator, inbox, and limits
./target/release/kit-deck status --json

# Generate a 15-key 5x3 Stream Deck MK.2 SVG + HTML preview sheet
./target/release/kit-deck preview --out /tmp/kit-deck-preview
./target/release/kit-deck preview --fixture --out /tmp/kit-deck-preview
```

Environment overrides: `KIT_DECK_DATA_DIR`, `KIT_DECK_HC`, `KIT_DECK_HERDR_APP`, `KIT_DECK_COORD`, `KIT_DECK_USAGE`.

## Actions (`dev.herdr.kit.*`)

| Action | Tap | Hold (0.65s) |
|---|---|---|
| **Herdr Overview** (`overview`) | Focus top-priority agent in Herdr | Force refresh |
| **Agent Inbox** (`inbox`) | Focus active inbox item's terminal | Advance to next item |
| **Inbox Action** (`inbox-action`) | `next` / `prev` / `focus` active item | Send quick reply (`reply_1` / `reply_2`) via `kit-coord say` |
| **Agent Slot** (`agent`) | Focus slot's agent terminal | Interrupt working agent (`hc interrupt`) |
| **Coordinator** (`coordinator`) | Focus coordinator terminal | Start / repair (`kit-coord up`) |
| **Usage Limits** (`limits`) | Select provider for bucket keys (or cycle if `worst`) | Force usage refresh |
| **Limit Bucket** (`limit-bucket`) | Force usage refresh | Force usage refresh |

## Recommended 5×3 Stream Deck MK.2 Layout

- **Row 0**: `Herdr Overview` | `Agent Inbox` | `Inbox Action (Next)` | `Coordinator` | `Agent Slot #1`
- **Row 1**: `Agent Slot #2` | `Agent Slot #3` | `Agent Slot #4` | `Inbox Action (Reply 1)` | `Inbox Action (Reply 2)`
- **Row 2**: `Limits (Codex)` | `Limits (Claude)` | `Limits (Copilot)` | `Limits (Antigravity)` | `Limit Bucket (Selected #1)`
