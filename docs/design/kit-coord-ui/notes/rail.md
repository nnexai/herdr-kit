# Variant 02 — `rail` · "Mission Log"

Mock: `docs/design/kit-coord-ui/02-rail.html` (single file, inline CSS/JS/SVG).
Open params: `?w=narrow` · `?freeze=1` · `?rm=1` · `?chrome=0` (hide mock toolbar); hash `#idle #typing #working #reply`.
Screenshots: run `evidence/kit-coord-ui/rail-render.sh` (see "Verification status" below).

## One-line pitch
The coordinator as a flight recorder: one glowing vertical rail, every journal entry a node on it, and the coordinator's own replies the biggest beads on the string.

## Concept
The window is a *timeline first*, chat second. A single rail runs down the left, colour-blended between consecutive nodes (each segment gradients from its node's colour into the next one's). Entries hang off the rail as cards via a short tick. A time gutter in mono sits left of the rail. The composer is a slim `›` command line at the bottom, not a chat bubble box. The person is reading a *log of what the system did*, with the conversation as one entry kind among others.

Node vocabulary (kind → colour → shape → weight):

| kind | colour | node | card |
|---|---|---|---|
| you (person message) | fg `#e6e7e9` | hollow diamond | tinted, `›` prefix, label YOU |
| coordinator reply | accent `#8fa9dc` | **big** 14px filled bead with ring + glow | raised card, markdown, chips |
| task placed | teal `#6cc4c9` (extension) | hollow ring | slim event card, link-like chips (tab/space) |
| delegate report | done `#6fcf97` | filled dot | slim event card, outcome line |
| progress | working `#e2b94e` | small filled dot | one-liner + 5-pip bar |
| decision noted | violet `#b79ae6` (extension) | small rounded square | quoted line with violet bar |
| question for the person | blocked `#ef6a67` | 12px dot, **pulsing ring** | red-tinted card, quick-reply pills |

Colour is never the only signal: every card carries a text kind label (QUESTION, PLACED, …) and node shapes differ.

## Hierarchy (top → bottom, reading order = DOM order)
1. Header: rail logo + "Coordinator · mission log", status line (running · role model · 2 active workstreams), `⋯` menu (Clear context · Restart · Restart now… with an in-menu confirm; all states from the project decisions).
2. Log (`role="log"`): rows = gutter time | rail+node | card. Last element is the **head**: the live node + status text.
3. "↑ 1 question waiting" pill, only when an unanswered question has scrolled off the top.
4. Composer: optional `↳ replying to question · 09:36 ×` context line, then `› [ input ] [Send ↵]`.
5. Footer: `Enter send · Esc close` (left), `journal · synced 09:36` (right, hidden narrow). Footer left doubles as the one-line notice slot (menu results, mock link clicks).

## Interaction & states
- **idle** — history shown, head node hollow grey, label "Coordinator idle · listening", sub-line "1 question is waiting for your answer" while one is pending. Log is bottom-anchored.
- **typing** — composer border goes accent with a soft halo, Send turns filled accent (enabled).
- **working** — Enter appends the person's row, composer clears, Send disables, placeholder says why, prompt glyph `›` turns amber. Head goes amber (see animation). Gutter shows a live elapsed counter (`0:02`). Sub-line cycles real-sounding steps (reading the journal → checking checkouts in herdr-kit → choosing a role for the task → placing on a worker) so the person sees *what* is happening, not just that something is.
- **reply** — after ~3.6 s the reply card slides in (10px rise + fade + one-shot border flash), its big node pops with a single ripple ring; ~0.5 s later the event row (task placed) follows the same way. Scroll follows smoothly. Head returns to idle.
- **Question chips** — clicking `Queue after commit` / `Hold` only **fills the composer** and adds the reply-context line; the person still presses Enter (or ×, or edits). On send the question node turns done-green, stops pulsing, collapses to "↳ you answered: …". The mock then returns a different coordinator reply + decision event (i6) for Queue vs Hold.
- Link-like chips (tab `t2v`, space `herdr-kit`) are buttons; mock only shows a notice. Production would reuse the existing focus-tab / focus-space bar actions.
- Keys: **Enter** sends, **Esc** closes the menu, otherwise closes the popup (mock: notice). No other bindings. Everything else is pointer or Tab.
- The mock sends canned replies regardless of typed text (only the Queue/Hold path differs).

## Working animation
- **Live node** at the rail head: amber core with glow, a thin amber ring, and one 5px dot orbiting the ring (2.4 s per turn, linear).
- **Flowing rail**: the ~135px of rail above the live node gets an amber gradient band (2px wide, fades out toward the top via mask) that scrolls downward at ~20px/s (3.2 s per 64px tile) — the rail "feeds" the node. The segment colour above it already blends into amber.
- Step text under the node crossfades (0.35 s) every 1.1 s; a blinking block caret (1.1 s, `steps(1)`).
- Nothing else moves while working; question pulse (1.9 s) continues independently if a question is pending.
- **Reduced motion** (`prefers-reduced-motion: reduce` via the media query and JS class, or `?rm=1`): no orbit, no shimmer, no pulse, no slide/pop/ripple, no smooth scroll, no caret blink. The indicator becomes a static amber ring (2px) on the live node, a static amber fade on the rail segment, the live elapsed counter and the text step. The orbit dot hides. Question pulse becomes a static ring.
- `?freeze=1` pins every loop to a fixed pose (orbit 38°, shimmer offset, step #2, elapsed 0:02) for deterministic screenshots; timed send sequence still works.

## Narrow (≈360px dialog, container query on the dialog width, not the window)
Time gutter 50→38px, rail column 30→24px; header wraps so the status line takes its own row ("2 active"); "mission log" subtitle and footer-right hidden; Send loses the `↵` glyph; chips wrap; no horizontal scroll (all text `overflow-wrap:anywhere`, ids ellipsize).

## Accessibility
- Dialog is a labelled `section`; log is `role="log"`, `aria-live="polite"`, `aria-relevant="additions"`, focusable (`tabindex=0`, visible focus ring) so keyboard users can scroll it.
- Head row is `aria-live="off"` (step text changes every 1.1 s must not be read); a separate visually-hidden `role="status"` announces "Coordinator is working" / "Coordinator replied".
- Rows are `<article>` with `<time datetime>`; kind label is real text.
- Focus order: ⋯ menu → log → question chips (and link chips) → reply-context × → input → Send. Menu: `aria-haspopup/expanded/controls`, `role=menu/menuitem`, Esc closes and returns focus to ⋯.
- Contrast: time gutter and footer use `#858990` (≈5:1 on `#101214`, ≈4.2:1 on `#24272b`) instead of `--dim` (`#6f7379`, ≈3.9:1 on page, below AA at 11px). Kind labels are the status colour on page/raised bg; `--blocked` on `#101214` ≈ 6:1. [INFERENCE: ratios estimated, not measured with a tool.]
- Pointer/keyboard parity: chips, pill, menu, ×, Send are all `<button>`s.

## Strengths
- Reads as *a record of what happened*, which is what the person actually wants from the coordinator; events are first-class, not decoration.
- Colour+shape+text kinds make a long log scannable; the one red pulsing node is the only thing demanding attention.
- The working state is informative (elapsed + current step) and spatially meaningful (flow lands on the head node).
- Chips fill the composer rather than send — no accidental irreversible answers; fits "UI is a window onto the journal plus a message input".

## Risks
- Left rail + gutter costs ~80px (≈62px narrow) of width before text; narrow cards are ~270px.
- Glow/blend effects are the most expensive part in gpui (see below); a cheaper tier (flat nodes, solid segments) must remain acceptable.
- Journal rows have to map to exactly the kinds above; unknown kinds need a neutral grey fallback node.
- Two extra palette colours (teal, violet) are introduced; if the family should stay at the existing 7, map placed→accent, decision→muted.
- Person cannot interject while the coordinator is working (Send disabled) — open question; alternative is queueing the message as a "queued" node.
- Bottom-anchored log hides history above the fold; the question pill mitigates only for questions.

## gpui implementation notes
Maps directly:
- Row = `h_flex` of fixed-width gutter `div` (mono text) + fixed-width rail `div` + card `v_flex`; whole log = `v_flex` in a `ScrollHandle` container, scrolled to end on append.
- Nodes = absolutely positioned `div` with `rounded_full`, bg colour, border, `shadow`; diamond = rotated square is **not** simple — use a `svg` path or a small square with `rounded_sm` (shape cue can drop to "square" without losing meaning).
- Rail segments = 2px `div` with solid colour per row (the colour-blend gradient needs `linear_gradient` background if gpui-kit exposes it; otherwise use the *next* node colour at 50% — degrades gracefully).
- Composer = `Input/InputState` in an `h_flex` with a mono `›` label and a `Button`; chips = small `Button`s; Esc/Enter handled by existing Input actions.
- Working animation: `cx.timer.every(33–50ms)` bumping a phase `f32`; orbit dot = absolutely positioned `div` at `(cos, sin)` offsets from the phase; rail shimmer = 4–6 small fixed `div` blobs along the segment whose y-offset/`.opacity` derive from phase (no gradient scrolling needed); step text = swap string every N ticks. `.frames(list,ms)` can drive the orbit as 8–12 pre-positioned frames instead of timers.
- Entry animation: `.opacity` stepped over 3–4 timer ticks + y-offset via `mt()`; skip the border flash/ripple if costly.
- Reduced motion: gate the timer behind the app's motion preference; render the static ring state.
Hard / risky:
- Glow (`box-shadow` blur 11–16px) — gpui `shadow` is limited; fake with one or two concentric low-opacity rounded divs.
- Mask-faded shimmer and gradient-above-gradient layering; CSS `mask-image` has no equivalent — approximate by per-blob opacity falloff.
- Smooth scroll-follow and "question scrolled off" detection need explicit ScrollHandle offset math (no IntersectionObserver).
- Inline markdown (bold/code) inside cards needs a small styled-text builder; the mock assumes it.
- Container queries do not exist; width breakpoints must be read from the dialog's bounds.

## Mod vs view lean
Leans **separate mod `kit-coord-ui`**: it is a timeline-first window with its own composer and journal projection, deliberately not a Board/Timeline tab (they are the inbox; this is the conversation surface). It would read the same journal/status state kit-coord already publishes (cached coord status key, journal) and call the existing CLI/mod commands for send, Clear context and Restart — it owns no lifecycle logic and does not import kit-coord code.

## Verification status (honest)
This session had no shell / process-execution tool (only read, grep, glob, write, edit), so the HTML was **not opened in a browser, no screenshots were rendered, and no JS was executed**. The mock was proof-read statically only. To produce the evidence PNGs run:

```
bash evidence/kit-coord-ui/rail-render.sh
```
It writes `rail-{idle,typing,working,reply}-{wide,narrow}.png` (960×680 / 380×640, frozen, toolbar hidden), `rail-working-anim-t1.png` / `-t2.png` (unfrozen at virtual 0.7 s and 1.9 s — orbit dot position, shimmer offset and step text must differ) and `rail-working-rm-{wide,narrow}.png` (reduced-motion). [INFERENCE: `--virtual-time-budget` advances CSS animations in headless Chrome; if t1 and t2 are identical, capture with playwright and `page.waitForTimeout` instead.]
Things most likely to need a visual pass once rendered: vertical alignment of node centre vs card first line (nodes sit at y=15px), tick length between node and card, the `.flow` band overlapping earlier rail segments, narrow header wrapping.
