# Coordinator dialog

One dialog replaces both the Mission Log dialog and the Board/Timeline popup. `mock.html` is the approved interactive mock. It shows the design as a right sidebar; for now it ships as a dialog with the same content. Open `mock.html` in a browser and use its demo bar to see every state.

## What it answers, in order

1. **Does anything need me?** Open questions and blocked delegates are pinned at the top. The pinned block is present only when something needs you.
2. **What happened to what I asked?** The stream shows your messages, the coordinator's replies, and under each message one live *receipt* per workstream that message started.
3. **What is the coordinator doing right now?** A quiet working line above the composer.

Everything else is record and lives in a receipt's trace or in the Journal view: rungs, rejected rungs, resources, nudges, models, brief and result paths, and watcher plumbing. The dialog does not repeat agent process status; Herdr's Agents list already shows it.

## Parts

- **Header**: "Coordinator" plus a state word (Idle, Working · 0:12, Off, Degraded), a ⋯ menu and a close ×. The menu holds Journal, Show raw coordinator, Clear context (hidden while off) and Restart. Restart is graceful; "Restart now" sits behind a confirmation while a restart is pending.
- **Needs you**: one compact block per item, on a faint red-tinted surface with a 2px red left edge. It shows the workstream title and age, the text clamped to 3 lines with "more", option chips and "Reply…". A chip sends the option as the answer (`say --question`). "Reply…" shows an "Answering: …" chip above the composer, removable with ×. Show at most 2 blocks, then "N more". Open questions come only from the authoritative `open_questions` projection. Blocked, stalled and start-blocked tasks come from the board's attention items. Linked questions are never shown twice.
- **Stream**: newest at the bottom; it starts scrolled to the bottom and follows new content while you are at the bottom.
  - Day separators read "Today", "Yesterday" or a date. Days before today collapse to one line, "Yesterday — 6 asks, all done", which expands on click.
  - **Your message**: a full-width raised block with the time at the right of its first line. It shows "sending…" until delivery is confirmed. No "You" label.
  - **Coordinator reply**: plain text on the dialog surface, with no bubble and no label.
  - **Receipt**: a workstream, anchored under the latest person message sent at or before the workstream opened. If there is no such message, it is placed at its open time. A hairline connector joins it to the ask. It is two lines:
    - Line 1: a state glyph, the title, then the space name and duration at the right.
    - Line 2: the one most useful sentence. Running shows the latest progress, or "Agent started in <space>". Done shows the closing outcome. Blocked shows what blocks it. A question shows "Waiting on you ↑".
    - Glyphs follow Herdr's Agents list: a ring spinner when running (static under reduced motion), green ✓ for done, yellow ! for blocked, red ? for needs you, and a hollow ring for placed.
    - On hover or selection, "Open terminal" and "Trace" replace the space and duration. Click toggles the trace: "HH:MM sentence" lines for that workstream only (placed with its plain why, started, progress, decisions "what — why", reports, closed), plus links: Open terminal, Show space. One level only.
  - **Echo line**: when a workstream reports done, blocked or a question after a later stream item exists, one muted line appears at that time. For example: "✓ Calendar view mocks finished — 3 mocks + spec". Its ↑ scrolls to the receipt.
- **Working line**: shown only while the coordinator runs a turn. It shows the step label and elapsed time. The motion is subtle and respects reduced motion.
- **Composer** (unchanged decisions):
  - Multi-line, auto-grows to about 6 lines, then scrolls.
  - Enter inserts a newline; only the small accent send icon inside the box at the bottom-right sends.
  - The icon is disabled when the text is empty or whitespace.
  - Single-line text and caret are centred.
  - Off: replaced by "Coordinator is off" with a Start button.
- **Journal** (from ⋯): replaces the stream, with a back arrow in the header. It has a filter input, "Show system events" (off by default), day groups and newest first. Rows read "HH:MM glyph sentence (workstream)"; click a row to show its raw fields. "Load older" pages backwards.

No keyboard shortcuts. Enter (composer newline, activating focused controls) and Esc (close the menu, then the dialog) are the only key handling.

## Entry points

- **Sidebar row**: the bottom-left row reads "Coordinator", with a small red count when something needs you. The count is read from the cached coord status in shared state. Clicking the row opens the dialog.
- **Status bar**: clicking the coordinator item opens the same dialog when the coordinator is running. When it is off or degraded, clicking starts or repairs it, as before.
- **Standalone `kit-coord`** (without `kit-coord-ui`): clicking its status item focuses the raw coordinator terminal when it is running.
