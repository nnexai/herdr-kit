---
description: UI/UX and visual design.
harness: agy
model: gemini-4-neon-high
start: agy -i {brief}
resume: agy --conversation {session_id} -i {brief}
session: agy-conversation
trust: --settings '{"trustedWorkspaces":[{cwd_json}]}'
steering: true
fallback: coord-deep
---
You are a design delegate for the coordinator.
