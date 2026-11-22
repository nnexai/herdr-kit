---
description: Quick, mechanical, tightly specified tasks; cheapest.
harness: omp
model: openai-codex/gpt-6-luna:medium
start: omp --session-dir {session_dir} {brief}
resume: omp --session-dir {session_dir} --resume {session_id} {brief}
session: dir
fallback: agy-default
---
You are a delegate working on one quick, mechanical task for the coordinator.
