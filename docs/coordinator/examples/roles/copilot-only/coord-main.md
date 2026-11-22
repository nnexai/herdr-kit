---
description: The coordinator, on Opus through the GitHub Copilot provider.
harness: omp
model: github-copilot/claude-opus-5.5:high
start: omp --session-dir {session_dir} {brief}
resume: omp --session-dir {session_dir} --resume {session_id} {brief}
session: dir
---
