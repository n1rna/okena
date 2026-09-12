---
name: knowledge-draft
description: Brief an agent to add to or update a knowledge root
for: knowledge-draft
---
Add to or update this knowledge base: {request}

You are in `{path}`, a {what}. Its layout:
- `docs/**/*.md` — engineering principles, processes, architecture and runbooks.
- `skills/<name>/SKILL.md` — one skill per directory, in the Agent Skills format (frontmatter `name` and `description`), with its supporting files beside it.
- `agents/<name>.md` — one subagent per file, in the Claude Code subagent format (frontmatter `name`, `description`, optional `tools` and `model`).
- `templates/**/*.md` — prompt templates: frontmatter `for:` lists the launch flows they apply to, and the body uses `{{placeholder}}` names.

Give every file frontmatter with a one-line `description` (plus `title` and `tags` for docs): people and agents choose entries by it. Read the existing entries first, and extend one rather than duplicating it. Keep it short and specific to how this team works, and ask me about anything ambiguous rather than inventing policy.{commit_note}

{>reporting}
