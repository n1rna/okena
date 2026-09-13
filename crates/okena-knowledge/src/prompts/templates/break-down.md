---
name: break-down
description: Brief an agent to split a task into sub-tasks through okena's MCP tools
for: break-down
---
Break {key} down into sub-tasks.

Title: {title}
Kind: {kind}
Link: {url}

Description:
{description|no-description}

Work through okena's MCP tools:
- `okena_list_subtasks` first, so you do not duplicate a child that already exists.
- `okena_create_task` once per child, with `parent` set to `{parent_id}`.
- `okena_update_task` or `okena_comment_task` on {key} itself, if the breakdown changes what it says or leaves something worth noting there.

Prefer several small children over one large one. Give each a short imperative title and say in its description what "done" means. A child is normally one step narrower than its parent — a {child_kind} under a {kind}. Ask me before inventing scope that is not implied by the parent.

{>reporting}
