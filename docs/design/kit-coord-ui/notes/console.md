# 04 · Refined Console (`console`)

Mock: `docs/design/kit-coord-ui/04-console.html` · capture script: `evidence/kit-coord-ui/console-capture.sh`

> **Verification status.** The author had no execution tool (read/grep/glob/write/edit only), so the mock was **not rendered or screenshotted**; no PNGs exist. Everything below describes what the HTML/CSS/JS is written to do, checked by reading only. Run `evidence/kit-coord-ui/console-capture.sh` to produce the 4 states x wide/narrow PNGs, two non-frozen animation frames (`console-anim-a/b.png`) and a reduced-motion frame, then review spacing/overflow before accepting the look. Unobserved claims are marked `[INFERENCE]`.

## Pitch
A command-palette prompt on top, the coordinator's journal underneath as typographic blocks hanging off a left label gutter. The coordinator is a single living status line, not a bubble.

## Concept
- **Spotlight prompt.** One soft gradient plate (no border at rest, 1px accent ring on focus) holds a `›` glyph in the gutter and a mono input that grows 1 to ~4 lines. `send ⏎` is a button that is accent-filled only when there is text; idle it is a dim `⏎`.
- **One gutter, three roles.** The `›` of the prompt, the `coord` label of the status line and the row labels (`you`, `coord`, `placed`, `done`, `ask`, `note`, `prog`) all right-align to the same edge (`--g`: 92px wide, 54px narrow). A faint vertical hairline at that edge fades downward. Text starts at the same x as the prompt text.
- **Newest first.** The prompt is at the top, so the stream grows downward away from it: the most recent turn sits directly under the status line. Inside a turn rows are chronological (`you`, `coord`, `placed`, `ask`). Bottom of the stream fades out with a mask.
- **Voice contrast.** Machine voice (labels, ids, events, the person's echo) is monospace. The coordinator's prose is sans 13.5px, with `code` pills for paths/roles. Event metadata is `muted`.
- **Status line = the coordinator's presence.** Always visible, same position, same row as the `⋯` menu:
  - idle: `● running · omp @coord · 2 workstreams active` (green dot; `omp @coord` is `[INFERENCE]` for "role model" in the brief).
  - working: braille spinner + phase text with a travelling highlight + elapsed seconds.
- **Questions** are an `ask` row with a pulsing lilac dot in the gutter, the prose, then an inline bracketed list `[ Queue after commit ]  [ Hold ]  or type a reply above`. Clicking resolves it in place to `↳ Queue after commit · answered` and starts a new turn with the answer echoed as `you`.
- **Events** (`placed`, `done`, `prog`, `note`) are one or two mono lines with ids in `fg`, links (`t2v`, `herdr-kit`) as dotted-underline accent text, and for `prog` a `■■■□□` meter. They are rows in the journal, never cards.
- Palette: the herdr-kit popup tokens verbatim, plus two extensions: `--ask #c6a8f0` (questions) and `--note #7cc4c0` (placed events). Label colours: you `fg`, coord `accent`, placed `note`, done `done`, ask `ask`, note `muted`, prog `working`.

## States (hash deep-links; `?freeze=1` freezes the frame)
| state | what shows |
|---|---|
| `#idle` | empty prompt with placeholder `Ask the coordinator…`, disabled send, status idle, 3 history turns (`prog` ws-q3m8d9 3/5, `done` ws-p7pfy2, `note` i5) |
| `#typing` | prompt holds the sample message (grows to 3 lines wide / ~5 narrow), `send ⏎` accent-filled, focus ring |
| `#working` | person's message echoed as `you` at the top; prompt empty, placeholder says you can draft the next message; send disabled; status line animates; amber glow behind the header breathes; rail under the prompt carries a sweeping amber highlight |
| `#reply` | `coord` prose in two lines, token row (task/role/tab/space), `placed` event row (flash), `ask` row with bracketed choices; status back to idle with workstreams 2 → 3 (tick) |

Live sequence (Enter, or click a choice): `you` row appears; phases `reading journal` (0s) → `choosing placement` (1.1s) → `placing ws-pyazfs-t1` (2.1s); at 3.0s reply lines arrive at 0 / 0.38 / 0.8 s (line 1, line 2, token row), `placed` at 1.3 s, `ask` at 1.9 s. Answering a choice runs a shorter script (`recording answer`, `queueing install`) and returns `Queued. …` plus a `prog` row. Any other typed text gets the same placement script (mock).

## Working animation
1. Braille spinner `⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏` at 80ms in `working` amber.
2. Phase text is muted with a pale-amber highlight band sweeping left to right (1.8s linear loop via `background-clip:text`); on phase change the text fades up 3px (240ms).
3. Elapsed seconds, dim, whole-second updates only.
4. Prompt chevron turns amber; 2px rail under the prompt carries a 30%-wide amber highlight sweeping (1.9s ease-in-out); a faint amber glow behind the header breathes (5s, opacity .5 to 1).
5. Reply text "types in" per line: each paragraph wipes in left-to-right with a 4px rise (500ms), lines staggered 380ms; event rows additionally get a fading accent wash (1.9s). No per-character animation.

**Reduced motion** (`prefers-reduced-motion: reduce`, or `?rm=1`, or the mock's `motion:` toggle): all CSS animation/transition is disabled by a single rule; spinner becomes a static `◐`; the shimmer becomes a static pale-amber text colour; the phase text still changes in place and elapsed seconds still count (text only); the reply and event rows are inserted instantly with all delays collapsed to zero; the ask dot stops pulsing; glow is a static amber. Information is not carried by motion alone: phase text and elapsed time say "working". `?freeze=1` additionally pins the spinner frame, shimmer position, sweep position and elapsed (3s) for deterministic screenshots.

## Narrow (<= 520px container width, e.g. 380px window or `?w=narrow`)
Same hanging structure, gutter 54px, labels 11px. Hidden: row timestamps, the word `send`, the word `workstreams`, footer right hint. Prompt font 14px/22px, up to 5 lines then internal scroll. Status line ellipsizes at the right; `⋯` stays. Token pills and bracketed choices wrap. Menu is `min(264px,100%)` wide, right aligned under the status line. Responsive via a container query on the dialog, so it responds to dialog width, not window width.

## Keyboard (no new bindings)
Enter sends. Esc closes the host popup (inside the mock Esc only closes the `⋯` menu if open; otherwise shows a hint). Everything else is Tab / Space / Enter on native buttons: prompt → send → `⋯` → bracketed choices and links in stream order. No arrow-key, number-key or Shift+Enter handling; the prompt wraps instead of taking manual newlines.

## Accessibility
- Stream: `role="log"`, `aria-live="polite"`, label "Coordinator timeline, newest first". Status line: `role="status"` (phase text is announced on change, prefixed by a visually hidden "coordinator working:"). `[INFERENCE]` phase announcements might be chatty with a real, faster-changing phase feed; announce only the first phase if so.
- Prompt is a labelled textarea; the `›` is `aria-hidden`; the send button has `aria-label`. Menu is an `aria-expanded` disclosure of plain buttons (`role="group"`), not an arrow-key `menu`, to avoid implying arrow bindings.
- Bracketed choices are real `<button>`s inside a `role="group"` labelled "Answer"; brackets are decorative spans that screen readers will still read, `[INFERENCE]` low impact, `aria-hidden` could be added.
- Focus: 2px accent outline (`:focus-visible`) on every button; the prompt shows the plate ring.
- Contrast (from tokens, computed by hand, not measured): `fg #e6e7e9`, `muted #9a9ea5` on `#101214` are well above 4.5:1. `dim #6f7379` is about 3.9:1 and is used only for timestamps, separators, brackets, the elapsed counter and the `or type a reply above` hint, none of which is the sole carrier of meaning. Label colours (accent, note, done, ask, working) are all above 4.5:1 `[INFERENCE]`.
- Colour is never the only signal: each row kind has a text label.

## Strengths
- Reads like the surface the person already lives in (terminal) but is clearly designed: gradients, type contrast, one gutter.
- The status line gives a stable, glanceable "what is the coordinator doing" without a chat bubble; phases are honest information, not decoration.
- Events and prose are visually separated by type and gutter label, so the timeline stays scannable as event density grows.
- Bracketed choices are a very low-chrome way to answer questions; typing a free answer still works.
- Smallest footprint of the four concepts: works as a compact popup or palette.

## Risks
- Newest-first with a top prompt inverts the usual chat order; people used to chat may expect the reply below the input at the bottom, and reading a multi-row turn top-down while the stream grows downward needs checking in use.
- Mono-heavy text reads dense for long coordinator answers; the sans prose mitigates but mixing two families needs taste in gpui font handling.
- The hanging labels cost 54 to 92px of width; narrow still works but prose columns are ~250px at 380px.
- Text shimmer (`background-clip:text`) is the hardest effect to reproduce in gpui (see below).
- Header-hero amber glow and mask fade may be dropped without harming the concept.
- Only 3 history turns were mocked; behaviour with a long journal (scroll position, jump-to-latest) is undesigned.

## gpui implementation notes
Maps directly:
- Layout: `v_flex` column; every row an `h_flex` with a fixed-width, right-aligned label `div` (92/54px) and a flexible body; prompt plate is the same grid with `›` in the gutter. Narrow switch = conditional on the dialog width.
- Braille spinner: `.frames(list, 80ms)` over the ten glyphs; static `◐` for reduced motion.
- Elapsed seconds and phase text: `cx.timer.every(1s)` updating a state field; phase text swap with `.opacity` fade.
- Prompt: `Input`/`InputState`; Enter submit is native. Send is a `Button`. Bracketed choices: ghost `Button`s with `[ label ]` text. Menu: small popover list with three `Button`s.
- Scroll: `ScrollHandle` on the stream; "newest first" means scroll offset 0 is the latest, no auto-scroll logic.
- Per-line reveal: `cx.timer` stepping each paragraph's `.opacity` 0 to 1 (and optionally a few px `top` offset); reply rows inserted in sequence from the journal poll, honoring reduced motion by inserting at once.
- Rail sweep: absolutely positioned small `div` whose left offset is advanced by a timer (or `.frames` of offsets).

Hard or approximated:
- **Text shimmer**: no gradient-clipped text; approximate by splitting the phase string into per-character spans and moving a 5-character highlight window by recolouring them from a timer (about 20 chars at 60ms; cheap). `[INFERENCE]` that per-span colour is supported by the text element.
- **Auto-growing input**: `[INFERENCE]` whether `Input` supports multi-line auto height; fallback is a fixed 2 to 3 line multi-line input that scrolls internally.
- **Clip-path wipe / translateY rise**: replace with opacity-only reveal (or a short width reveal on a clipped container if overflow-hidden containers allow it).
- **Radial glows and the bottom mask fade**: gradient backgrounds are `[INFERENCE]` available (linear only); drop radial glows, use a linear gradient or flat page colour, and a flat bottom fade overlay `div` in the page colour.
- **Inline rich text** (bold, `code` pills, mixed sans/mono) in the coordinator prose needs a styled-run text element or small hand-rolled inline flow; pills inside wrapped paragraphs are awkward. Fallback: render `code` as mono-coloured spans without background.
- The `:focus-within` ring on the plate needs explicit focus-state styling.

## Mod vs view lean
Separate mod `kit-coord-ui` shown as a compact popup/palette. It would read the coordinator journal/status through the same shared state the Board and Timeline use and send the message through the existing coordinator input path, owning no lifecycle (Clear context / Restart reuse the existing coordinator dialog commands). It can also live as a view inside kit-coord if the mod split stays undecided; nothing in the concept depends on being a separate package, but the compact palette form factor is the argument for its own entry point. Not decided here.

## Open questions
- Is the journal source able to stream the working *phase* ("reading journal", "choosing placement")? If not, the status line falls back to a generic "working" with elapsed time and a shimmer. Default: build for phases, degrade gracefully.
- Can the person send while the coordinator is working (queue), or is send blocked as in the mock? Default: allow drafting, block sending, with the placeholder saying so.
- "role model" in the status brief is rendered as `omp @coord`; confirm what the status item should say.
- Where does Clear context sit relative to the existing dialog action? Mock puts it, Restart and Restart now… in the `⋯` menu.
