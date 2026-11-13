# Variant 01 — "Quiet Concierge" (`conversation`)

Mock: `docs/design/kit-coord-ui/01-conversation.html` · screenshots: `evidence/kit-coord-ui/conversation-*.png` (see **Verification status** — not rendered yet).

**Pitch:** The coordinator talks to you like a concierge: unhurried editorial prose in the middle of the dialog, your words as soft pills, the system's bookkeeping reduced to slim ticker rows, one floating composer with a faint glowing edge.

## Concept

Conversation-first. A single centred column (max 640 px) inside the existing Coordinator dialog shell. The coordinator's voice is the only large type on screen; everything else (events, chips, timestamps) is quiet UI chrome. The dialog gets a third tab, **Talk**, next to Board and Timeline; Board + Timeline remain the inbox, Talk adds no list or queue of its own.

```
┌ Coordinator  [Talk][Board][Timeline]        ● running · role model · 2 active  ⋯  Close ┐
│ ───────────────────────────── TODAY ─────────────────────────────                      │
│  ✓ ws-p7pfy2 closed  delivered                                            13:41        │
│    coord-design role (omp @design) added, … 011b0de deployed live and verified        │
│  ◉ COORDINATOR 13:41                                                                    │
│  That one is closed. UI and visual design now has its own lane…   (serif 17px)         │
│  ◆ Decision noted · i5                                                    13:42        │
│  ╭ ◉ COORDINATOR 14:06                         ● QUESTION FOR YOU ╮                    │
│  │ The popup edits in mods/herdr-kit-coord are uncommitted …       │                   │
│  │ (Queue after commit)  (Hold)                                    │                   │
│  ╰─────────────────────────────────────────────────────────────────╯                   │
│                          ╭──────────────────────────────────────────╮                  │
│                          │ one more idea: kit-coord-ui instead of … │  ← soft pill     │
│                          ╰──────────────────────────────────────────╯                  │
│  ●●● → ● reading journal… 0:04                                  ← thinking line        │
│   ╭──────────────────────────────────────────────────────────────╮ ← glowing hairline │
│   │ Tell the coordinator what you need…                      (↑) │                     │
│   │ Enter to send                                      Esc closes │                    │
│   ╰──────────────────────────────────────────────────────────────╯                     │
└─────────────────────────────────────────────────────────────────────────────────────────┘
```

## Visual hierarchy (top → bottom importance)

1. Coordinator prose — serif stack (`Iowan Old Style / Palatino / Noto Serif / DejaVu Serif`), 17 px/1.62 (16 px narrow), `**bold**` rendered 700 white, `code` as small mono tag.
2. The open question — only element with a tinted card (`blocked` at 7 % → 2 % wash, 1 px `blocked` 26 % border) and a "Question for you" badge, plus two quick-reply pills (first = accent outline, matches `.btn.first` in `evidence/popup-consistency/design-mock.html:28`).
3. Person message — right-aligned pill, accent 13 % fill, radius 18/18/6/18, sans 14 px.
4. Ticker rows — 22 px tinted icon square (done = `done`, decision/placed = `accent`, progress = `working`), sans 12.5 px, ids in mono, optional detail line, timestamp right (hidden < 640 px). Consecutive tickers sit 8 px apart instead of 20 px so they read as one cluster.
5. Chips — 20 px pills; link-like chips (task, role, tab, space) accent-tinted buttons, plain facts (harness, model) neutral spans.
6. Composer — 20 px-radius floating pill on `input` (#2a2d31) with a 1 px accent gradient hairline on its top edge (13 %–87 % width, blurred twin underneath).

## States

| State | What shows |
|---|---|
| idle | History (day label, delivered ticker, coordinator note, decision ticker, open question). Composer empty, hairline 50 %, send button disabled grey. Hint "Enter to send · Esc closes". Question visible → no "waiting" pill. |
| typing | Composer text present: border accent 55 %, hairline 95 %, 3 px accent halo, textarea auto-grows to 112 px, send becomes accent filled with soft glow. |
| working | Person pill appended; composer cleared, placeholder "Coordinator is working…", hint "Coordinator is working · you can keep typing", halo ring breathes, header chip turns `working` yellow and pulses, send disabled. Thinking line in the thread (below). |
| reply | Thinking line leaves (200 ms), reply rises in (opacity + 10 px + blur 3 → 0, 550 ms), `task placed` ticker follows +450 ms, `delegate progress` (3 of 5 segmented bar) +1100 ms, each ticker flashing accent 20 % → neutral over 2.2 s. If the open question has scrolled out of view, a "1 question waiting for you ↑" pill appears above the composer; click scrolls to it. |

Live interaction (not just hash states): type + Enter → working for ~5.9 s (phase labels `reading journal…` → `checking roles…` → `placing…` → `writing reply…`) → reply. Typing the exact demo sentence yields the sample reply; any other text yields a generic ack. Clicking a quick-reply chip answers the card in place ("You answered **Queue after commit**"), appends the pill, runs a turn, and ticks a decision row. Header `⋯` menu: **Clear context** (dims the whole past to 42 % and draws a "Context cleared" hairline + "Fresh session" line) and **Show coordinator terminal** (the raw TUI stays one click away; never hidden).

Send while working is disabled (text stays editable) — see open questions.

## Working animation

- **Composer halo:** a 1 px accent ring + 30 px soft glow breathes opacity 0.3 → 0.95 over 3.4 s ease-in-out; the top hairline breathes 0.55 → 1 in the same rhythm.
- **Thinking line:** three accent dots bob (−4 px, opacity 0.35 → 1, 1.25 s, 160 ms stagger). After ~0.9 s the dots *morph*: they shrink to 78 % with a tighter gap and the italic serif phase label expands out of zero width next to them. Each phase swap crossfades (6 px rise + blur) while a slow text sheen sweeps the label (2.6 s). A mono elapsed counter `0:04` sits at the end.
- Header chip dot pulses yellow.
- Nothing moves on the page itself; all motion is opacity/transform on small elements.

**Reduced motion** (`prefers-reduced-motion: reduce`, or `?rm=1`, or the demo-bar checkbox): no dots, no breathing, no entry animations, no sheen, no transitions. A static yellow "calm" dot and a label "Working · reading journal…" (text updates every 1.3 s without animation; this is a text change, not motion) plus a static halo ring at 60 % opacity. New rows appear in place. `?freeze=1` is a screenshot aid (animations off, dots shown at fixed staggered opacities, label pre-expanded); it is separate from reduced motion.

## Narrow (< 640 px dialog width; container query, same breakpoint as the popup's two-row header — `docs/coordinator/board-design.md:211`)

- Header becomes two rows: `Coordinator · tabs · Close` / `● running · 2 active … ⋯`; "role model" dropped.
- Dialog padding 14/12; column padding 2 px; prose 16 px; pills up to 90 % width; ticker timestamps and "Esc closes" hidden; quick replies wrap; composer radius 18.
- Checked by construction only (see Verification status): at 360 px the header row budget is ~318 px of 334 px; `flex-wrap` is the fallback if a font renders wider.

## Accessibility

- Dialog `role="dialog" aria-label="Coordinator"` (as `ui-board.js:53`); tabs `role="tablist"`.
- Thread is `role="log"` (implicit polite live region, additions only), `tabindex=0`, labelled "Conversation with the coordinator". Each message/event is an `article` with an `aria-label` ("Coordinator, 14:09", "Event: ws-p7pfy2 closed, delivered, 13:41"; question adds ", question for you").
- The animated thinking line is `aria-hidden`; a separate off-screen `role=status` announces only "Coordinator is working." and "Coordinator replied." (no per-phase chatter).
- Focus order: tabs → Close → ⋯ menu → thread (scroll) → question quick replies → chips → "question waiting" pill → textarea → Send. Textarea is focused on open. Menu: opens focus on first item, Esc closes and returns focus; arrow-key roving inside the menu is not implemented in the mock.
- Quick replies are real buttons (30 px high); chips that act are buttons, facts are plain spans. Send has `aria-label="Send message"`.
- Contrast (approximate, computed from token luminances, [INFERENCE] not measured with a tool): fg #e6e7e9 and serif #eceef1 on #24272b > 11:1; muted #9a9ea5 ≈ 5.5:1; added `--faint` #868b92 for timestamps/hints ≈ 4.2:1 (borderline AA for 11 px — risk); `dim` #6f7379 (≈ 3.1:1) is **not** used for text. Question badge `blocked` #ef6a67 on the red wash ≈ 4.4:1 at 10.5 px caps — borderline.
- Colour is never the only carrier: question has a text badge, answered state has text + check, event kind has icon + words.
- Reduced motion honoured as above; no auto-scrolling animation under reduced motion.

## Shortcuts

None added. Enter sends, Esc closes (host). The textarea keeps its native behaviour; Tab order is native. The Board/Timeline footer hints (`n clear context`, etc.) are not shown on Talk — the old 28 px footer row is replaced by the composer's hint line (deliberate deviation from the shared shell, see risks).

## Strengths

- Most legible way to read the coordinator's reasoning; reads like correspondence, not logs.
- Events are demoted but never lost: they sit exactly where they happened in the story.
- The only hard attention signal is the one thing that needs the person (question), and it stays reachable via the waiting pill.
- Calm: motion is limited to the composer edge and one line of text.

## Risks

- Vertical density is low: a long journal day will need much scrolling; the ticker rows compete with prose when events are bursty (mitigation: 8 px cluster spacing; future: collapse ≥ 4 consecutive tickers into "4 events ▾").
- Deviates from the shared shell (no 28 px footer, Clear context moved to ⋯, third tab). Needs acceptance against the popup-consistency rule that Board/Timeline keep identical shells.
- Serif depends on installed fonts; DejaVu Serif fallback is wider and less refined.
- "Working" and the phase labels need a real signal; the journal may not expose phases (see gpui notes).
- Disabled send while working is a product decision, not evidenced.

## gpui implementation notes

Maps directly:
- Shell/header/tabs/column/pills/chips/ticker rows/question card: `div/h_flex/v_flex` with `.bg/.border_1/.rounded/.px/.py/.gap`; tokens already exist as `theme()` values used by `ui-board.js`.
- Thread: `v_flex` inside a scroll container with a `ScrollHandle`; pin to bottom after append (mock's `scrollTo(bottom)`). Auto-follow only when already at bottom is advisable.
- Composer: `Input`/`InputState` + `Button` (round send). Enter-to-send needs the input's submit event. Multi-line auto-grow: [INFERENCE] unknown whether `InputState` supports multi-line; if not, single-line with horizontal scroll, losing the "pill grows" feel.
- Dots: `.frames(list,ms)` cycling 4–6 frames (each frame sets the three dots' opacity/offset) at ~120 ms gives a stepped bob; label crossfade as two stacked divs with `.opacity` driven by `cx.timer.every`.
- Composer halo breathing: `cx.timer.every(~50 ms)` updating a phase → `.opacity(f)` on a ring div (1 px accent border + a second 3 px lower-alpha ring). Reduced-motion: skip the timer, set a constant opacity.
- Thinking line label swap: plain text change; the elapsed counter via the same timer at 1 s.
- `⋯` menu: existing overlay/popover pattern if present, else an absolutely positioned `v_flex`.
- Question quick replies: Buttons; the reply path to the coordinator (stdin/`coord` subcommand) is [INFERENCE] — not verified in this review.

Hard / needs a compromise:
- Soft glow (30 px blur box-shadow) and the blurred twin hairline: gpui shadows are limited [INFERENCE]; substitute 2–3 concentric 1 px rings at decreasing alpha and a 3-step stacked hairline.
- Gradient hairline: if no linear-gradient fill, use 3 abutting segments (transparent→accent) or a solid 1 px accent at reduced opacity.
- Blur-in on entry (`filter:blur`) and `background-clip:text` sheen: no equivalent; keep opacity + 8 px y-offset only; drop the sheen (use the dots as the only moving element).
- Top fade mask on the scroller: overlay a 26 px `popover`→transparent gradient div (or hard cut).
- Inline markdown (bold/code) in prose: needs a tiny parser producing styled text runs; serif font availability via font-family fallback [INFERENCE].
- Data: transcript = journal events (`coord log --json`, polled every 15 s per `board-design.md:425`) + coordinator prose. A "working" phase signal is not shown by `coord board/log`; may need a new status field or terminal scrape [INFERENCE]. Without it the thinking line can only say "working…" without phase labels.
- Auto-scroll + IntersectionObserver ("question waiting" pill): compute from ScrollHandle offset vs. the question's measured y.

## Mod-vs-view lean

**View inside kit-coord's dialog** (third tab "Talk"): it reuses the shell, the coordinator status chip, Clear context/restart machinery and the 15 s journal polling that already live in `mods/herdr-kit-coord`. Only if kit-coord must be includable alone in another layout *without* a chat would a separate `kit-coord-ui` mod be preferable; the data layer would still have to be shared.

## Open questions (recommended defaults)

1. Send while working: queue (show "queued" tag under the pill) or block? **Default: block send, keep text** (as mocked) until the coordinator's turn model is known.
2. Tab order: should Talk be the default tab on open? **Default: yes when the coordinator is running, Board otherwise.**
3. Chips: navigate (focus tab / open space) or copy id? **Default: navigate, with tooltip.**
4. Ticker burst handling: collapse ≥ 4 consecutive rows? **Default: yes, "N events ▾".**
5. Question lifecycle: answered cards stay in place (as mocked) vs. move to Timeline only. **Default: stay.**

## Acceptance scenarios

1. Open `#idle` at 960×680: day label, delivered ticker, coordinator note, decision ticker, open question card with two quick replies; composer empty, send disabled; no waiting pill.
2. `#typing`: composer shows the 150-char sentence over two lines, border/hairline brighter, send accent with glow.
3. `#working`: person pill above a thinking line with dots + label "reading journal…" (cycling every 1.3 s), composer ring visibly breathing, header chip says "working" in yellow, waiting pill visible if the question is out of view.
4. `#reply`: reply with 4 chips, then "placed" ticker with tab/space/harness/model chips, then progress ticker with 3/5 bar; waiting pill visible.
5. Type any text + Enter at `#idle`: pill appears immediately, dots 450 ms later, label ~0.9 s later, reply ~5.9 s after Enter, then tickers staggered.
6. `?rm=1` (or Chrome `--force-prefers-reduced-motion`): no dots, no breathing; yellow dot + "Working · …" label; reply and tickers appear without motion.
7. `?w=narrow` at 380×640: header in two rows, no horizontal overflow anywhere, composer fully visible, ticker rows wrap detail text.
8. Click "Queue after commit": card turns into "You answered …", turn runs, decision ticker appears, waiting pill disappears.
9. `⋯ → Clear context`: all earlier rows dim, "Context cleared" divider + "Fresh session" line appear.

## Verification status (honest)

The sub-agent that produced this variant had **no shell/browser tool** (only read/grep/glob/web_search/write/edit), so the page was **never opened in a browser and no screenshots were rendered**. HTML/CSS/JS were reviewed by reading only; layout numbers above (e.g. narrow header budget) are estimates. To produce and inspect the required PNGs run:

```
bash evidence/kit-coord-ui/conversation-shots.sh
```

It writes `conversation-{idle,typing,working,reply}-{wide,narrow}.png` (frozen), `conversation-working-wide-anim-t1.png` / `-t2.png` (animation on, two virtual times — the animation proof: compare ring intensity, dot offsets and phase label), and `conversation-working-{wide,narrow}-rm.png` (Chrome `--force-prefers-reduced-motion`). Expect to iterate on: narrow header wrapping, serif font fallback metrics, question card vertical rhythm, and the `.edge` blurred twin.
