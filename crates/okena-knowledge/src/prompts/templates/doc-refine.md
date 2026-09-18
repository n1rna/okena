---
name: doc-refine
description: Brief an agent to change one open document at the user's request
for: doc-refine
model: sonnet
---
Change `{file}`: {request}

It is {what} at `{path}`, in `{root_path}`. Read it first, and enough of what sits around it to keep it consistent with the rest — but edit only this file. If doing this properly would mean changing other files too, stop and tell me which rather than editing them.

Keep its structure and frontmatter unless the request is about them. Ask me about anything you would otherwise have to guess at. Do not commit: I review the change and commit it myself.{context}

{>context-lookup}

{>reporting}
