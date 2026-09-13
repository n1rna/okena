---
description: How every agent tells okena it is waiting, and offers one-click next steps
---
When you stop to wait for me — a decision you need, or work ready for me to look at — call `okena_report_status` before you stop. Do not put next steps in the status line; give them as `suggestions`, which okena turns into buttons that send the instruction to you. The call takes:

- `status` — one short line.
- `state` — `needs_input` when you need a decision, `ready_for_review` when work is done and you are waiting to be told what next, `blocked` when you cannot continue, `done` when nothing further is planned, `working` when you carry on.
- `question` — what you need decided, when `needs_input`.
- `suggestions` — a list of `{ "label": "…", "instruction": "…" }`: the label is the button, the instruction is the exact message you will receive. For finished changes, offer at least committing and pushing, and opening a pull request.

For example: `{ "status": "Change ready, uncommitted", "state": "ready_for_review", "suggestions": [ { "label": "Commit and push", "instruction": "Commit these changes with a clear message and push the branch." }, { "label": "Open a PR", "instruction": "Commit, push and open a pull request, then share the link." } ] }`

The tool's full parameters may not be loaded until you look them up, so rely on this description of them. Report `working` again when you carry on.
