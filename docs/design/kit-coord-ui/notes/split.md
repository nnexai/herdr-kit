# Variant 03 `split` — "Control Room"

Mock: `docs/design/kit-coord-ui/03-split.html` (single file, inline CSS/JS/SVG, no network).

## Verification status (read first)

This variant was authored in a session that had **no shell and no browser** (tools: read/grep/glob/web_search/write/edit). Therefore:

- **No screenshots were rendered** and nothing in `evidence/kit-coord-ui/` besides this file exists for `split`. The HTML was proof-read line by line but **never executed**; treat layout details (spacing, narrow overflow, the exact `@container` threshold) as unverified until someone looks at renders.
- No two-frame animation proof exists. The animations are plain CSS keyframes (below); the proof is two screenshots of `#working` taken ~700 ms apart without `freeze`.
- To produce the required images (run from the repo root; `bare=1` hides the demo toolbar so the dialog fills the window; `freeze=1` makes the shot deterministic):

```sh
H=docs/design/kit-coord-ui/03-split.html; O=evidence/kit-coord-ui
for s in idle typing working reply; do
  google-chrome --headless=new --hide-scrollbars --window-size=960,680 --screenshot=$O/split-$s-wide.png   "file://$PWD/$H?bare=1&freeze=1#$s"
  google-chrome --headless=new --hide-scrollbars --window-size=380,640 --screenshot=$O/split-$s-narrow.png "file://$PWD/$H?bare=1&freeze=1&w=narrow#$s"
done
# animation proof: freeze OFF, two shots with a virtual-time budget difference
google-chrome --headless=new --window-size=960,680 --virtual-time-budget=300 --screenshot=$O/split-working-wide-t1.png "file://$PWD/$H?bare=1#working"
google-chrome --headless=new --window-size=960,680 --virtual-time-budget=1100 --screenshot=$O/split-working-wide-t2.png "file://$PWD/$H?bare=1#working"
# reduced motion: ?rm=1 (same code path as the OS preference)
google-chrome --headless=new --window-size=960,680 --screenshot=$O/split-working-wide-calm.png "file://$PWD/$H?bare=1&rm=1#working"
```
- Things most likely to need a visual fix on first render: lane card heights in the right panel at 960x680 (the reply state is the densest; the lane list scrolls if it overflows), the question card + 4 other cards in the 400 px chat column, and the pill strip text at 360 px.

## Pitch

Chat on the left, a live "what is it doing now" control panel on the right. The person talks to the coordinator on the left and watches it think and act on the right; every event card in the chat points at the workstream lane it touched.

## Concept

Two panes inside the existing coordinator dialog shell (padding 18, header 24, footer 28, same tokens). The header gets a third tab **Chat** next to the existing Board / Timeline tabs (the mock only draws Board/Timeline as inert tabs); this is a view of the same dialog, not a new inbox.

- **Left, chat column (~400 px max).** Person messages are right-aligned bubbles on `selected`. Coordinator replies are plain text with a small hexagon avatar and hang off a 1 px **spine**; every event (task placed, delegate done, decision noted, delegate progress, question for you) is a compact card with a node on that spine, a colour-coded left rule, and a `lane ↗` chip. So reply + its events read as one thread, which is the "response timeline".
- **Right, "Now" panel (dot-grid backdrop on `page` colour, so it reads as an instrument, not another card).** Top: a 3-segment **phase stepper** — Reading inbox → Choosing placement → Starting delegate — and a one-line mono **activity log** under it that swaps text per phase. Below: **Workstreams as lanes**: 3 px state edge (yellow running, red needs you, accent placing, green delivered), title + mono id + state word, task chips (`ws-…-t1 ● running`), a meter or placement meta (`tab t2v · space herdr-kit · omp @design`), and for blocked lanes an inline "Needs you" strip with `see question ↙`. Closed lanes collapse into a dim one-line "Closed today" section.
- **Linking.** Clicking `lane ↗` on a card (or the task chip in a reply) scrolls to and rings that lane (static outline in reduced motion). Hovering/focusing an event card tints its lane. `see question ↙` on a lane jumps back to the question card. This is the signature interaction of the variant.
- Header shows the coordinator state chip (`Running`, or `Working` in the working colour while a turn runs — `coordinator.status` already carries `working`, see `docs/coordinator/board-design.md` 4.4.1), `role model · N active`, a `Coordinator ⋯` menu (Clear context, Restart coordinator, Show coordinator terminal) and Close. The raw agent stays one menu item away; it is never hidden.

## Hierarchy

1. Composer + latest reply (left, bottom).
2. The active phase segment (right, top) — the only saturated moving element when working.
3. Lanes needing the person (red edge, sorted first), then running/placing, then closed.
4. Event cards (secondary weight, 12 px).

## States (all reachable from the toolbar and from `#hash`)

| State | What shows |
|---|---|
| `#idle` | History (person ask, reply, placed card, progress card 3/5). Panel: stepper at rest (3 neutral bars), activity line "idle · journal seq 4821 …", 2 lanes running, nothing pulses. Send disabled. |
| `#typing` | Same + sample text in the textarea (auto-grows to 4 lines), Send enabled, accent border on the field. |
| `#working` | Person message appended, status chip `Working`, working row at end of chat (orbit-ring avatar + "choosing placement" + three dots), stepper step 2 active (step 1 done, fill at 62 %), activity line shows the matching reasoning, a dashed ghost lane `ws-pyazfs` with a skeleton shimmer appears at the top, composer top edge sweeps, Send disabled (input stays editable: "you can keep typing"). |
| `#reply` | Stepper all green ("Done in 3.2s · 1 placed · 1 closed"), reply + placed + done + decision + question cards, lanes: `ws-k3m2ab` needs you (red edge, "Needs you" strip), `ws-pyazfs` running on `t2v`, `ws-p7pfy2` moved to "Closed today". |

**Real run:** type anything in idle/typing and press Enter (or click Send): user bubble appears and the composer clears → phase 1 (900 ms, `p7pfy2` edge pulses = the report being read) → phase 2 (1.2 s, ghost lane `pyazfs` appears) → phase 3 (1.1 s, ghost becomes solid "starting…" with placement meta) → at ~3.3 s working row disappears and the reply arrives; then cards land at +450 ms (placed, `pyazfs` goes running), +850 ms (delegate done, `p7pfy2` closes into the closed section), +1250 ms (decision i5), +1700 ms (question + `k3m2ab` turns red). Total ≈ 5 s. A second typed message runs a short "nothing to place" turn (steps 2–3 shown as skipped/dashed). The question's **Queue after commit / Hold** chips run a short turn too and update the lane note and clear the red strip. All data is scripted; nothing leaves the page.

## Working animation (subtle, tied to meaning)

- Active stepper segment: fill grows linearly across the phase duration, with a pale highlight band sliding along the filled part (`background-position` loop, 1.3 s) and a faint glow.
- Active lane (the one the coordinator is touching right now): the 3 px edge pulses (opacity 1 → .3, 1.5 s) with a soft coloured glow and a slight tinted background. Running-but-untouched lanes keep a steady edge, so pulse = "now".
- Chat: working row with a slowly orbiting dashed ring around the avatar and a three-dot wave; a 2 px yellow sweep along the composer's top edge; header chip dot blinks.
- Arrivals: cards slide in 8 px with a one-shot ping on their spine node; lanes drop in / flash their state colour once.
- **Reduced motion** (`prefers-reduced-motion: reduce`, also `?rm=1` and the "reduce motion" checkbox): every `animation`/`transition` is disabled by one rule on `.calm`. Information is carried statically: the active step is filled to 50 % in solid yellow with an outline, the label is bold and the panel header says "Choosing placement · step 2 of 3"; the pulsing lane gets a static tinted outline and full-opacity edge; ring, sweep and dot-wave are removed or static; lane/card highlight after `lane ↗` is a 2.2 s static outline.
- `?freeze=1` is the screenshot mode: same CSS rule disables motion but keeps the stepper highlight band frozen mid-bar.

## Narrow behaviour (dialog < 640 px, i.e. container content width ≤ 602 px)

- Header splits in two rows exactly like the existing popup (title/tabs/Close, then status/meta/menu).
- The right panel leaves the layout and becomes a **pill strip** above the chat: mini 3-segment progress, phase text ("Choosing placement · step 2 of 3" / "Standing by …"), and "2 active · 1 needs you" on a second line, chevron. 44 px high, full width; text ellipsizes, nothing overlaps.
- Tap/Enter on the pill opens the panel as a **sheet** pinned under the pill over a dimmed scrim (same DOM, `aria-expanded`), with a "Hide panel" bar at its foot. Esc, scrim tap, or Hide closes it and returns focus to the pill. Tapping `lane ↗` in the chat opens the sheet already scrolled to and ringed on that lane. `see question ↙` closes the sheet and jumps to the question.
- Stepper sub-labels, event timestamps, the footer's right-hand text and the "Enter sends" hint are dropped; chips wrap.

## Accessibility

- Roles: dialog `Coordinator`; tabs have `role=tab`/`aria-selected`; chat log `role=log` (focusable, `aria-live=off`) with a separate visually hidden `role=status` polite region announcing phase changes ("Coordinator: choosing placement") and "Coordinator replied. One question needs you."; the stepper is an `ol` with `aria-current="step"` on the active item and a hidden state word per item (pending/in progress/done/skipped); the lane list is `role=list`; event cards are `article` with an `aria-label`; the meter has `role=img` and "3 of 5"; the pill is a button with `aria-expanded`/`aria-controls`; the ⋯ menu uses `role=menu`/`menuitem`.
- Focus order: header (tabs, Close, menu) → pill (narrow) → chat log → chips/buttons inside cards (lane, tab/space links, quick replies) → composer textarea → Send → panel (Hide button in the sheet). The textarea gets focus on open.
- Colour never carries state alone: lanes always print the state word (`running`, `needs you`, `placing…`, `delivered`), steps print ✓/–/number, the question card is labelled "Needs you".
- Contrast: body text `fg`/`muted` on `raised`/`page` ≥ 4.5:1 (est.). `dim` (#6f7379) is used only for timestamps, hints and ids [INFERENCE: ≈ 3.9:1 on `page`, below AA for small text; acceptable only because nothing essential depends on it — review].
- Keyboard: only Enter to send and Esc to close (menu → sheet → dialog, in that order). No new shortcuts.

## Strengths

- Makes the coordinator's *work* legible, not just its words: the person sees which lane is being touched while a reply is still being written.
- Cards and lanes cross-link, so the chat doubles as an index into the board without a separate inbox.
- Reuses board data (lanes = board workstreams/tasks) and journal events (cards = timeline entries); the panel is basically a live Board; very little new data.
- Narrow mode is a real design (pill + sheet), not just a squeezed pane.

## Risks

- Heaviest of the four variants on screen real estate; two panes at ≥ 640 px are tight and the lanes can scroll in the densest state.
- The phase stepper assumes the coordinator exposes its phase. Today `coord status` only says `working|idle`; phases must be derived from journal events or by watching the agent (see gpui notes). If the derivation is unreliable the stepper can lie — fallback is a single indeterminate segment labelled "Working".
- Duplicated information (event card + lane) can feel busy; mitigated by keeping cards one-liners and lanes compact.
- Cards that reference a lane the board no longer lists (closed long ago) need a graceful "lane archived" state (not drawn).
- Unverified visuals (see status above).

## gpui implementation notes

Maps directly:
- Shell, header, tabs, footer, chips, lane cards, task chips, event cards: `div/h_flex/v_flex` with `theme()` tokens exactly like `ui-board.js` / `ui-timeline.js`; 3 px edge = a 3 px-wide child div with `bg(color)`.
- Chat log: `v_flex` + `ScrollHandle` + `overflow_y_scrollbar`, scroll to end after append via the handle; right-aligned bubble = `h_flex().justify_end()`.
- Composer: `Input.new(InputState.new({placeholder}))`, `on('submit', …)` for Enter, `Button` for Send (`.disabled`), same pattern as the timeline filter input.
- Stepper segments: three `div`s with `bg`/`rounded`; the fill is a child with a percent/px width computed from a `cx.timer.every(100, …)` elapsed value.
- Pulsing edge / blinking dot: `cx.timer.every(…)` toggling `.opacity(…)` between two or three values (or `.frames([...], ms)` for a 6-step opacity ramp); working dots: `.frames(['', '.', '..', '...'], 350)` as text.
- Working orbit avatar: 4–8 pre-rotated `svg` frames via `.frames(list, ms)`.
- Reduced motion: gate every timer/frames behind one `calm` flag (setting or OS if exposed); static states already designed.
- Pill + sheet: sheet is an absolutely positioned `v_flex` over the chat with a scrim div (`on_click` closes); breakpoint from `on_size_change`, same as the popup's 640 px switch.

Hard / needs care:
- **Moving highlight band** inside the filled segment (CSS background-position loop): no gradient animation; approximate with a 24 px translucent white child div whose `left` advances on the timer and is clipped by the fill (requires `overflow_hidden` on the track), or drop it and keep only fill + glow.
- **Glow** (`box-shadow` blur): probably unavailable or limited; use a lower-alpha 6 px wider div behind the edge.
- **Slide/ping arrival animations**: no keyframes; emulate arrival with a short `opacity` ramp (3–4 steps) on the new item only, skip translate.
- **Container queries**: replace with width state from `on_size_change`.
- **Auto-growing textarea**: `Input` is likely single-line; keep one line and ellipsize, or allow a fixed 2-line height if multiline input exists (`[INFERENCE]`, not confirmed in the gpui-kit surface listed in board-design.md).
- **Smooth scroll / scrollIntoView for lane highlight**: use `ScrollHandle.scroll_to_item(i)` (instant) + a 1.5 s timer for the ring (border colour toggle).
- **Spine line** through variable-height rows: draw as a `border_l(1)` on each row's left gutter div rather than one absolute line.
- **Phase data**: derive phase from journal events since the last person message (e.g. inbox read → placement decision → `task.placed`/delegate start) via `coord log`; no model calls, same polling/`watchFolder` rules as the board.

## Mod vs view lean

**A view inside kit-coord** (third tab "Chat" in the existing Coordinator dialog). It reuses `coord board`/`coord log`/`coord status` data, the shell skeleton, the header actions (Clear context/Restart) and the one-poller-per-kind rules; a separate mod would duplicate all of that and need its own copy of the board decoder. The only new data path is sending a message to the coordinator, which the popup's current "What the mod must never do" list in `docs/coordinator/board-design.md` forbids (it must never send text to a terminal), so it needs an explicit new coord command. If kit-coord is split into layouts, this stays with it.
