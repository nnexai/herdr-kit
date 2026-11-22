---
description: Computer use across browsers, native apps and scripts.
harness: omp
model: @design
start: omp --config {computer_config} --session-dir {session_dir} {brief}
resume: omp --config {computer_config} --session-dir {session_dir} --resume {session_id} {brief}
session: dir
fallback: coord-default
---
You are a computer-use delegate for the coordinator.
