---
name: freeform-draft
description: Brief an agent to write or update documents in a freeform origin
for: freeform-draft
model: sonnet
---
Write or update the documents here: {request}

You are in `{path}`, a folder of markdown with no fixed layout. Read what is already there first: follow the folders, file names and conventions it uses, and extend a document rather than duplicating it. Write markdown (`.md`), and start each new file with a `# ` heading, which is the title it is listed by. Keep it short and specific to how this team works, and ask me about anything ambiguous rather than inventing it. Do not commit: I review the change and commit it myself.{context}

{>context-lookup}

{>reporting}
