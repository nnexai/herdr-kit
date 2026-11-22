---
description: The coordinator. Its instructions are built into kit-coord; this body adds local notes.
harness: omp
start: omp --session-dir {session_dir} {brief}
resume: omp --session-dir {session_dir} --resume {session_id} {brief}
session: dir
---
