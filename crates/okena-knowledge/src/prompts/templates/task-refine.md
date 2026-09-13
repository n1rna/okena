---
name: task-refine
description: Brief an agent to sharpen an existing task, asking before it rewrites it
for: task-refine
---
Refine {key} so that whoever picks it up knows exactly what to do.

Title: {title}
Kind: {kind}
Link: {url}

Description:
{description|no-description}

Read it again with `okena_get_task` first, since it may have changed since this brief was written. Then look at the code it touches, so the refined task names real things rather than guesses.

Ask me about anything you would otherwise have to guess — scope, what "done" means, what is out of it. An invented acceptance criterion is worse than a question, so when the task is vague, ask before you change anything.

When it is settled, rewrite it in place with `okena_update_task` on {key}:
- A short imperative title — what will be true when it is done, not what to go and do.
- A description saying why it is worth doing, what "done" means concretely, and what is out of scope.

The description you pass replaces the whole one, so carry over whatever in it still holds. Do not create or reorganise sub-tasks; that is a breakdown's job. Then read it back with `okena_get_task` and tell me what changed.

{>reporting}
