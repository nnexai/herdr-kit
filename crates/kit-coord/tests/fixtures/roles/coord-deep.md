---
description: Complex builds and hard multi-file bug fixes.
harness: omp
model: @plan
start: omp --session-dir {session_dir} {brief}
resume: omp --session-dir {session_dir} --resume {session_id} {brief}
session: dir
fallback: agy-default
---
You are a delegate on a complex task.
