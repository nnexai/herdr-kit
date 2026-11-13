# kit-coord-ui: chat front-end variants (ideas only)

Idea: instead of looking at the raw coordinator omp TUI, the person gets a simplified chat input plus a response timeline. Coordinator replies are interleaved with key events (task placed, delegate reported, question for the person, decision noted). A subtle "working" animation shows while the coordinator runs a turn. No product code, no keyboard shortcuts beyond Enter (send) and Esc (close).

Each mock is a single self-contained HTML file with every state, the working animation, and a narrow layout. Open one and use the toolbar, or type and press Enter for a real idle -> typing -> working -> reply run.
URL controls: `#idle|#typing|#working|#reply`, `?w=narrow|wide`, `?freeze=1` (stop animation), `?rm=1` (reduced motion), `?bare=1`/`?chrome=0` (hide the demo toolbar). `prefers-reduced-motion` is honoured.

| # | Mock | Name | Idea | Lean |
|---|---|---|---|---|
| 01 | `01-conversation.html` | Quiet Concierge | Editorial serif coordinator voice, right-aligned person pills, slim event tickers, floating composer | view in kit-coord (a "Talk" tab) |
| 02 | `02-rail.html` | Mission Log | Glowing vertical event rail; replies are big nodes, events small; questions pulse red with quick-reply chips | separate mod |
| 03 | `03-split.html` | Control Room | Chat left, live "now" stepper plus workstream lanes right; collapses to a pill + sheet when narrow | view in kit-coord (a "Chat" tab, reuses board data) |
| 04 | `04-console.html` | Refined Console | Spotlight-style prompt, hanging gutter labels, braille spinner + shimmering phase line, line-by-line reply reveal | separate mod (compact palette) |

Per-variant specs (states, animation, reduced motion, narrow, accessibility, gpui notes): `notes/<name>.md`.

## Comparison

| | Strengths | Risks | gpui / Herdr mod notes |
|---|---|---|---|
| 01 Conversation | Most approachable and "fancy"; calm; reads like a chat people know | Closest to a generic chat clone; events can feel secondary; serif widens text | Mostly div/Input/Button. Breathing glow = `.opacity` on a `cx.timer.every` tick; dot morph via `.frames`. Auto-growing input and rich inline text are the hard parts |
| 02 Rail | Strongest identity; questions and events scan instantly; ties to the Timeline tab | Rail/node geometry and flowing gradient are fiddly; denser | Rail = absolute-positioned divs; gradient flow needs stepped opacity bands, not a true gradient animation |
| 03 Split | Shows what the coordinator is doing now; best for "why was it placed there" | Needs phase data that `coord status` does not expose (derive from journal); widest, most to build | Reuses board lanes; container queries do not exist, so the narrow switch is `on_size_change` |
| 04 Console | Most distinctive; compact; fits the terminal-adjacent culture | Monospace-heavy; reduced discoverability for non-terminal moods; text shimmer hard | Spinner via `.frames`; per-line reveal via `cx.timer`; text shimmer and auto-grow input are hard |

## Recommendation

Prototype **02 Rail** first if only one: it is the most distinctive and also doubles as the Timeline, so no second inbox view appears. If the lowest build risk matters more, **01 Conversation** as a "Talk" tab in the existing dialog. 03 is the richest but depends on new phase data; 04 is a good compact-palette alternative. Variants can be combined (e.g. 01's composer with 02's rail).

## Open points (all variants)

- Behaviour when the person sends while the coordinator is mid-turn (the mocks keep typing enabled or block it; nothing in the repo confirms the real semantics).
- Answering a question via chip: the mocks fill or send the composer; real delivery is a coordinator-inbox message, not task management.
- Extra palette colours (placed teal, ask violet) are proposals; the real mod reads `theme()`.

## Verification

Rendered with headless Chrome: all four states at 960x680 and 380x640 per variant, plus two unfrozen working frames at different times (pixel-different for every variant, so the animation moves) and a reduced-motion frame. Screenshots (ignored): `evidence/kit-coord-ui/<slug>-<state>-<wide|narrow>.png`. Not verified: real gpui rendering, real coordinator data, interactive click-through beyond the scripted sequences, measured contrast.
