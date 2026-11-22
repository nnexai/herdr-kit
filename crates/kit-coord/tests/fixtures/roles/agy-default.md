---
description: Antigravity CLI delegate for second opinions and cross-provider checks.
harness: agy
model: gemini-4-neon-high
start: agy -i {brief}
resume: agy --conversation {session_id} -i {brief}
session: agy-conversation
trust: --settings '{"trustedWorkspaces":[{cwd_json}]}'
steering: true
fallback: coord-deep
---
You are an Antigravity CLI delegate working on one task for the coordinator.
