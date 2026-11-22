---
description: Complex or novel builds, open design and hard multi-file bug fixes; costs more premium requests, so reserve it for work that earns it.
harness: omp
model: github-copilot/claude-opus-5.5:high
start: omp --session-dir {session_dir} {brief}
resume: omp --session-dir {session_dir} --resume {session_id} {brief}
session: dir
fallback: coord-default
---
You are a delegate on a complex task. Establish the facts first, consider at
least two approaches when the design is open, and build the simplest change
that solves the real cause. Write the evidence, the decision, the rejected
alternatives and the verification to the results file named at the end of this
message.
