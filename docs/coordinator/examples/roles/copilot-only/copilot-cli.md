---
description: GitHub Copilot CLI delegate for a second opinion from another harness; no mid-turn steering.
harness: copilot
model: auto
start: copilot --session-id {session_id} -i {brief}
resume: copilot --resume {session_id} -i {brief}
session: preset
fallback: coord-default
---
You are a delegate working on one task for the coordinator. Keep the change
inside the brief's scope, verify with the brief's acceptance check and put long
results in the results file named at the end of this message.
