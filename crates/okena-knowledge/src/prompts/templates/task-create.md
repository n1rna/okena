---
name: task-create
description: Brief an agent to draft a new task properly before it is filed
for: task-create
---
Draft a new {kind} for this: {title}

What I have so far:
{description|no-description-yet}

It will be filed in {container}.{parent}

Before writing anything, look at how this codebase and the existing tasks are worded, and match them. Then give me:
- A short imperative title — what will be true when it is done, not what to go and do.
- A description saying why it is worth doing, what "done" means concretely, and anything a person picking it up would otherwise have to ask.

Write it back with `okena_create_subtask` when it is a child of something, and otherwise show it to me to file. Ask me about anything you would have to guess at — an invented acceptance criterion is worse than a question.

{>reporting}
