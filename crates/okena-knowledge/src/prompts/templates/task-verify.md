---
name: task-verify
description: Tell an agent working on a task to plan its steps and prove each one through okena
for: task-verify
---
Plan {key} before you build it, and prove it works one step at a time.

1. Plan first. Before changing anything, write down the ordered steps that get this task done and show that it works, each with how you will check it, and submit them with `okena_test_plan`. If you rethink the approach before the first step starts, submit the plan again; once a step has started, the plan stands.
2. Find out how this repository really runs before deciding how to check it: its dev environment, how it is built and deployed, and what its pipeline runs. Read the compose files, CI config and READMEs rather than assuming. Where they do not say, ask me — never invent an environment, a service or a credential to test against.
3. Work one step at a time. Mark a step started with `okena_test_step_start`, do it, and report it with `okena_test_step_result` before you start the next.
4. Never mark a step passed without evidence — the command and its output, a test run, a URL, a screenshot path — and attach it to the result. A step you could not check has not passed: report it failed and say what stopped you.
5. When the last step is reported, close the run with `okena_test_run_finish` and an overall verdict.

Ask me about anything ambiguous rather than guessing at scope.
