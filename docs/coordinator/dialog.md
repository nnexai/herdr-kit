# Coordinator dialog

`kit-coord-ui` shows one dialog for talking to the coordinator and reviewing its work. It reads only the coordinator CLI (`status`, `conversation`, `log`, `board --all`), and it sends messages only with `say`.

## What it answers, in order

1. **Does anything need me?** Open questions and blocked delegates are pinned at the top, and only while there are any.
2. **What happened to what I asked?** The stream shows your messages, the coordinator's replies, and a live *receipt* per workstream under the ask that started it.
3. **What is the coordinator doing right now?** A quiet working line above the composer.

Everything else is record: rungs, resources, nudges, models, paths and watcher plumbing. It lives in a receipt's trace or in the Journal. The dialog does not repeat agent process status, because Herdr's Agents list already shows it.

## Opening it

The coordinator item in the bottom bar is the only entry point.

- **Running coordinator:** left-click opens the dialog. Right-click opens it in any state.
- **Off or degraded:** left-click starts or repairs the coordinator.
- **Needs you:** the item turns to its attention colour. When the counts do not fit its 80px, it keeps the needs-you count (`● 1`).
- **Mounting:** `kit-coord-ui` is mounted as an invisible 0px block that receives the `show` command; the bar sends it through its `coord_ui_mod` prop.
- **Standalone `kit-coord`** (without the dialog): clicking its status item focuses the raw coordinator terminal.

## Parts

- **Header**
  - The title "Coordinator" and a state word: Idle, Working · 0:12, Off or Degraded.
  - A ⋯ menu with Journal, Show raw coordinator, Clear context and Reduce motion. Clear context is the graceful restart; it is hidden while the coordinator is off. While a restart is pending, the menu instead offers "Restart now…" behind a confirmation.
  - A close ×.
- **Needs you**
  - One block per item: the workstream title, age, the text (clamped to 3 lines), option chips and "Reply…".
  - A chip sends that option as the answer (`say --question`). "Reply…" puts an "Answering: …" chip above the composer.
  - At most two blocks show at once.
  - Questions come only from the authoritative `open_questions` projection. Blocked, stalled and start-blocked tasks come from the board's attention items. Linked questions are never shown twice.
- **Stream** (newest at the bottom; it follows new content while you are at the bottom, and "Jump to latest" returns there)
  - **Days:** separators read Today, Yesterday or a date. Earlier days collapse to one toggle line, e.g. "▸ Mon Nov 16 — 8 workstreams, all done".
  - **Your message:** a raised block with its time. Messages come from the current session's conversation and from journal `coord.sent` entries, so asks sent from the dialog survive coordinator restarts. Asks typed into the raw terminal show only for the current session.
  - **Coordinator reply:** plain markdown text on the dialog surface.
  - **Receipt:** one per workstream that has `ws.opened` in the loaded journal or appears on the board; board data wins.
    - Placement: it hangs under the latest person message at most 10 minutes before the workstream opened, with no other message in between. Otherwise it stands alone at its open time.
    - Layout: two lines. Line one has a state glyph (spinner, ✓, !, ?), the title, the space and the duration. Line two is one truncated sentence: progress, outcome, the blocker or "Waiting on you ↑".
    - Interaction: hover shows "Open terminal" and "Trace" in the same fixed-size slot as the space and duration, so the row never shifts. A click toggles the trace, which lists that workstream's events and its terminal and space links. A date separator (Today, Yesterday or a date) precedes the first event of each day.
  - **Echo line:** a workstream gets at most one echo. It is a one-line "✓ title — outcome" that appears only when a person message or reply lies between the receipt and its latest done, blocked or question report. Its ↑ jumps to the receipt.
- **Working line and composer**
  - **Working line:** the coordinator's real turn step and elapsed time. The pulse stops under reduced motion.
  - **Composer:** multi-line. Enter inserts a newline; only the send icon sends, and it is disabled when the text is empty.
  - **Off:** the composer is replaced by "Coordinator is off" and a Start button.
  - **Notices:** delivery confirmations fade after a few seconds; failures stay.
- **Journal** (from ⋯, back arrow to return)
  - Newest first, grouped by day, then by runs of one workstream whose title is shown once.
  - Each row is "time · glyph · sentence" on one line, in plain language with no tiers, rungs or ids. Click a row for the full sentence, the reason, the task and raw fields.
  - "System events" (off by default) adds watcher, inbox, resource, nudge, turn and coordinator-session events.
  - "Load older" pages backwards.

There are no keyboard shortcuts. Enter (newline in the composer, or activating a focused control) and Esc (close the menu, then the dialog) are the only key handling.
