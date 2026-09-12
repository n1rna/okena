---
name: spec-draft
description: Brief an agent to fill in a scaffolded OpenSpec change
for: spec-draft
---
Draft an OpenSpec change for this idea: {idea}

The change directory already exists at `{change_dir}` with its `.openspec.yaml` and a stub `proposal.md` holding the idea. Work only inside that directory.

Follow OpenSpec conventions (https://github.com/Fission-AI/OpenSpec):
- `proposal.md` — why this change, and what changes.
- `design.md` — the technical approach, when the change needs one.
- `tasks.md` — an implementation checklist.
- `specs/<capability>/spec.md` — delta specs for the requirements this change adds, modifies or removes.

Read the existing `openspec/specs/` before proposing. Prefer plain Markdown and keep it short. Ask me about anything ambiguous rather than inventing requirements.{store_note}{references}

{>reporting}
