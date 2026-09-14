//! okena's MCP server — stdio JSON-RPC, for agents okena is running.
//!
//! Transport choice: stdio rather than HTTP. Agents already know how to launch
//! a stdio MCP server from their own config, and stdio lets this reuse the
//! CLI's existing daemon auth wholesale instead of implementing MCP's
//! streamable-HTTP transport and a second authentication path.
//!
//! Identity is implicit and that is the point: the server reads
//! `$OKENA_TERMINAL_ID` from its own environment, which okena sets on every PTY
//! it spawns. An agent therefore cannot report against a session other than the
//! one it is running in — there is no session parameter to forge. A server
//! launched outside an okena terminal has no identity and refuses the tools
//! that write.
//!
//! Wire up with, in the worktree the agent runs in:
//!
//! ```json
//! { "mcpServers": { "okena": { "command": "okena", "args": ["mcp"] } } }
//! ```

use serde_json::{Value, json};
use std::io::{BufRead, Write};

/// MCP revision this server implements.
const PROTOCOL_VERSION: &str = "2024-11-05";

/// JSON-RPC error codes used here (per the spec's reserved range).
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

/// Run the server until stdin closes.
pub fn run() -> i32 {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            // stdin closed or went away mid-read: the agent is gone.
            Err(_) => break,
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let request: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                // Parse errors carry no id, so there is nothing to respond to;
                // log to stderr, which is not part of the protocol stream.
                eprintln!("okena mcp: could not parse request: {e}");
                continue;
            }
        };

        // A notification has no `id` and takes no response — replying to one
        // corrupts the stream.
        let Some(id) = request.get("id").cloned() else {
            continue;
        };
        let method = request.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let params = request.get("params").cloned().unwrap_or(Value::Null);

        let response = match dispatch(method, &params) {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err(err) => json!({ "jsonrpc": "2.0", "id": id, "error": err }),
        };

        if writeln!(stdout, "{response}").is_err() || stdout.flush().is_err() {
            break;
        }
    }
    0
}

fn rpc_error(code: i64, message: impl Into<String>) -> Value {
    json!({ "code": code, "message": message.into() })
}

fn dispatch(method: &str, params: &Value) -> Result<Value, Value> {
    match method {
        "initialize" => Ok(json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "okena", "version": env!("CARGO_PKG_VERSION") },
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tool_definitions() })),
        "tools/call" => call_tool(params),
        other => Err(rpc_error(
            METHOD_NOT_FOUND,
            format!("unknown method: {other}"),
        )),
    }
}

fn tool_definitions() -> Value {
    json!([
        {
            "name": "okena_whoami",
            "description":
                "Identify the okena session this agent is running in: terminal, \
                 project, path, and the task it was started for (if any). Call \
                 this first — the other tools act on this session.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
        },
        {
            "name": "okena_list_projects",
            "description":
                "List okena's projects with their paths, branches and worktrees, \
                 including the other worktrees created for the same task.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
        },
        {
            "name": "okena_report_status",
            "description":
                "Report what this agent is doing and whether it needs the user. \
                 Shown on the agent's panel in okena, and used to flag agents \
                 that are waiting on someone. Replaces the previous report. \
                 Call it whenever you stop to wait: with `needs_input` and a \
                 `question` when you need a decision, or `ready_for_review` when \
                 work is done and you are waiting to be told what next. Offer \
                 the likely next steps as `suggestions` — for finished changes, \
                 typically committing and opening a pull request — so the user \
                 can pick one instead of typing it. Report `working` when you \
                 carry on.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "status": { "type": "string", "description": "Short status line." },
                    "state": {
                        "type": "string",
                        "enum": ["working", "needs_input", "ready_for_review", "blocked", "done"],
                        "description":
                            "Why you are where you are. Anything but `working` or \
                             `done` flags you as waiting on the user."
                    },
                    "question": {
                        "type": "string",
                        "description": "What you need decided, when `needs_input`."
                    },
                    "suggestions": {
                        "type": "array",
                        "description":
                            "Next steps the user can send you with one click. The \
                             instruction is typed into your prompt verbatim.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "label": { "type": "string", "description": "Short button text." },
                                "instruction": {
                                    "type": "string",
                                    "description": "The exact message you will receive."
                                }
                            },
                            "required": ["label", "instruction"],
                            "additionalProperties": false
                        }
                    }
                },
                "required": ["status"],
                "additionalProperties": false
            }
        },
        {
            "name": "okena_list_subtasks",
            "description":
                "List the sub-tasks of a task — every child, including ones \
                 assigned to somebody else. Defaults to this session's own task. \
                 Call before creating children so you do not duplicate one that \
                 already exists.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "task": {
                        "type": "string",
                        "description":
                            "Provider id of the parent. Defaults to this session's task."
                    }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "okena_list_containers",
            "description":
                "List the teams or projects a new top-level task can be filed in, \
                 on this session's task provider.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
        },
        {
            "name": "okena_create_task",
            "description":
                "File a task on this session's task provider (Linear, Azure \
                 DevOps). Top-level with `container`, or a child with `parent`. \
                 Without either, the task goes in your only team — or, when you \
                 have several, the result lists them and asks which: pick one \
                 and call again.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "title": { "type": "string", "description": "Short imperative title." },
                    "description": {
                        "type": "string",
                        "description": "Markdown: why it matters and what \"done\" means."
                    },
                    "kind": {
                        "type": "string",
                        "enum": ["epic", "feature", "story", "task", "defect"],
                        "description": "Where it sits in the breakdown. Defaults to task."
                    },
                    "container": {
                        "type": "string",
                        "description":
                            "Team or project for a top-level task: its id, key \
                             (`QBL`) or name. Ignored with `parent`."
                    },
                    "parent": {
                        "type": "string",
                        "description":
                            "Id or key of the parent, to file a child in the \
                             parent's team."
                    }
                },
                "required": ["title"],
                "additionalProperties": false
            }
        },
        {
            "name": "okena_get_task",
            "description":
                "Read a task with its description and state. Defaults to this \
                 session's own task.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "task": {
                        "type": "string",
                        "description":
                            "Id or key (`QBL-371`, `#42`). Defaults to this session's task."
                    }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "okena_update_task",
            "description":
                "Change a task's title or description. Fields you leave out are \
                 left as they are; the description you pass replaces the whole \
                 description, so read it first with `okena_get_task`.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "task": {
                        "type": "string",
                        "description": "Id or key. Defaults to this session's task."
                    },
                    "title": { "type": "string", "description": "New title." },
                    "description": {
                        "type": "string",
                        "description": "New description, in Markdown. Empty clears it."
                    }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "okena_set_task_state",
            "description":
                "Move a task to a state. The provider picks its own column in \
                 that category, and the result shows which.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "task": {
                        "type": "string",
                        "description": "Id or key. Defaults to this session's task."
                    },
                    "state": {
                        "type": "string",
                        "enum": ["backlog", "todo", "in_progress", "in_review", "done", "canceled"]
                    }
                },
                "required": ["state"],
                "additionalProperties": false
            }
        },
        {
            "name": "okena_comment_task",
            "description":
                "Comment on a task — progress, a decision, a question for whoever \
                 reads it next.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "task": {
                        "type": "string",
                        "description": "Id or key. Defaults to this session's task."
                    },
                    "body": { "type": "string", "description": "The comment, in Markdown." }
                },
                "required": ["body"],
                "additionalProperties": false
            }
        },
        {
            "name": "okena_create_subtask",
            "description":
                "Create a sub-task under a task, in the same team as its parent. \
                 Use this to break work down: one call per child. Prefer several \
                 small children over one large one, and say in the description \
                 what \"done\" means for that child. Same as `okena_create_task` \
                 with `parent` defaulting to this session's task.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "title": { "type": "string", "description": "Short imperative title." },
                    "description": {
                        "type": "string",
                        "description":
                            "What the child covers and what finishing it means."
                    },
                    "kind": {
                        "type": "string",
                        "enum": ["epic", "feature", "story", "task", "defect"],
                        "description":
                            "Where it sits in the breakdown. Defaults to task. A \
                             child is normally narrower than its parent: a feature \
                             under an epic, a story under a feature."
                    },
                    "parent": {
                        "type": "string",
                        "description":
                            "Id or key of the parent. Defaults to this session's task."
                    }
                },
                "required": ["title"],
                "additionalProperties": false
            }
        },
        {
            "name": "okena_start_work",
            "description":
                "Start an agent on one or more tasks, each in worktrees of its own. \
                 Use this after deciding how the work splits: one call per \
                 group of tasks that can be built and tested together. Every \
                 task in the group gets its own worktree in each repo, on its \
                 own branch, and the agent is told which worktree is whose.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "tasks": {
                        "type": "array",
                        "items": { "type": "string" },
                        "minItems": 1,
                        "description":
                            "Provider ids or keys of the tasks in this group. The \
                             session is named after the first."
                    },
                    "note": {
                        "type": "string",
                        "description":
                            "What this group is for, and what the neighbouring \
                             groups are handling, so the agent does not go looking \
                             for work that is somebody else's."
                    },
                    "agent": {
                        "type": "string",
                        "description":
                            "Agent to launch. Omit for okena's configured default."
                    }
                },
                "required": ["tasks"],
                "additionalProperties": false
            }
        },
        {
            "name": "okena_register_asset",
            "description":
                "Record something this agent produced — a pull request, branch or \
                 document — against its session. Appends; call once per asset. \
                 Branches and pull requests of the task's worktrees are detected \
                 without this; registering one anyway (by `url`, or by `branch` \
                 and `project`) gives it your title.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "kind": {
                        "type": "string",
                        "enum": ["pull_request", "branch", "document", "other"],
                        "description": "What kind of thing was produced."
                    },
                    "title": { "type": "string", "description": "Human-readable label." },
                    "url": { "type": "string", "description": "Link to it, when there is one." },
                    "project": {
                        "type": "string",
                        "description":
                            "Repo it landed in, when the agent spans several projects."
                    },
                    "branch": {
                        "type": "string",
                        "description": "Branch it is on, when it is on one."
                    }
                },
                "required": ["kind", "title"],
                "additionalProperties": false
            }
        },
        {
            "name": "okena_test_plan",
            "description":
                "Plan how you will verify your work, before you run anything: submit \
                 the ordered steps of this session's test run, each with a title and \
                 what passing it proves. okena shows the plan in Harness → Testing as \
                 soon as you send it, then every step as it advances, so plan first \
                 and report as you go. Until a step starts you can call this again to \
                 replace the plan. Once one has started it is rejected, so the steps \
                 already run keep their results — finish the run with \
                 `okena_test_run_finish` and plan a new one instead.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "steps": {
                        "type": "array",
                        "minItems": 1,
                        "description": "The plan, in the order you will run it.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "title": {
                                    "type": "string",
                                    "description": "What you will do: \"Run the auth test suite\"."
                                },
                                "proves": {
                                    "type": "string",
                                    "description":
                                        "What passing it shows: \"Expired tokens are refused\"."
                                }
                            },
                            "required": ["title", "proves"],
                            "additionalProperties": false
                        }
                    }
                },
                "required": ["steps"],
                "additionalProperties": false
            }
        },
        {
            "name": "okena_test_step_start",
            "description":
                "Mark a step of your test run as running, just before you run it, so \
                 whoever is watching sees which step is live. Steps are numbered from \
                 1, in plan order. Starting a step that already has a result runs it \
                 again and clears that result — use it to re-verify after a fix.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "step": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "The step's number in the plan, from 1."
                    }
                },
                "required": ["step"],
                "additionalProperties": false
            }
        },
        {
            "name": "okena_test_step_result",
            "description":
                "Record a step's outcome once it has run: `passed`, `failed` or \
                 `skipped`, with a one-line `reason`. For a failure the reason is shown \
                 in place of a bare red mark, so say what went wrong, specifically. \
                 Attach evidence whenever you have it — the last lines of the output \
                 that decided the step, a URL you checked, a screenshot's path. A \
                 result without evidence is a claim; with it, the user can check.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "step": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "The step's number in the plan, from 1."
                    },
                    "outcome": {
                        "type": "string",
                        "enum": ["passed", "failed", "skipped"]
                    },
                    "reason": {
                        "type": "string",
                        "description":
                            "One line: why it passed, what failed, or why it was skipped."
                    },
                    "log_tail": {
                        "type": "string",
                        "description":
                            "The last lines of the output that decided the step — not the \
                             whole log."
                    },
                    "url": {
                        "type": "string",
                        "description": "A page or CI run that shows the result."
                    },
                    "screenshot_path": {
                        "type": "string",
                        "description": "Absolute path of a screenshot you took."
                    }
                },
                "required": ["step", "outcome", "reason"],
                "additionalProperties": false
            }
        },
        {
            "name": "okena_test_run_finish",
            "description":
                "Close this session's test run with an overall verdict — `passed`, \
                 `failed`, or `inconclusive` when the run could not settle it — and a \
                 short summary. Steps you never reached are marked skipped. Call it when \
                 you are done verifying, including when you stop early; an \
                 `okena_test_plan` after it starts a fresh run.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "verdict": {
                        "type": "string",
                        "enum": ["passed", "failed", "inconclusive"]
                    },
                    "summary": {
                        "type": "string",
                        "description": "What the run established, in a sentence or two."
                    }
                },
                "required": ["verdict"],
                "additionalProperties": false
            }
        }
    ])
}

/// Wrap a payload in MCP's tool-result envelope.
///
/// Content is JSON rendered as text: models read it reliably and it keeps the
/// structure intact, which a prose summary would lose.
fn tool_result(value: &Value) -> Value {
    let text = serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string());
    json!({ "content": [{ "type": "text", "text": text }] })
}

/// Report a tool-level failure.
///
/// MCP distinguishes protocol errors from tool errors: a tool that fails should
/// return `isError` so the model can read the reason and adjust, rather than a
/// JSON-RPC error which it never sees.
fn tool_error(message: impl Into<String>) -> Value {
    json!({
        "content": [{ "type": "text", "text": message.into() }],
        "isError": true
    })
}

fn call_tool(params: &Value) -> Result<Value, Value> {
    let name = params
        .get("name")
        .and_then(|n| n.as_str())
        .ok_or_else(|| rpc_error(INVALID_PARAMS, "missing tool name"))?;
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    let outcome = match name {
        "okena_whoami" => whoami(),
        "okena_list_projects" => list_projects(),
        "okena_report_status" => report_status(&args),
        "okena_list_subtasks" => list_subtasks(&args),
        "okena_list_containers" => list_containers(),
        "okena_create_task" => create_task(&args),
        "okena_create_subtask" => create_subtask(&args),
        "okena_get_task" => get_task(&args),
        "okena_update_task" => update_task(&args),
        "okena_set_task_state" => set_task_state(&args),
        "okena_comment_task" => comment_task(&args),
        "okena_start_work" => start_work(&args),
        "okena_register_asset" => register_asset(&args),
        "okena_test_plan" => test_plan(&args),
        "okena_test_step_start" => test_step_start(&args),
        "okena_test_step_result" => test_step_result(&args),
        "okena_test_run_finish" => test_run_finish(&args),
        other => {
            return Err(rpc_error(
                METHOD_NOT_FOUND,
                format!("unknown tool: {other}"),
            ));
        }
    };

    Ok(match outcome {
        Ok(value) => tool_result(&value),
        Err(message) => tool_error(message),
    })
}

// ─── Session identity ────────────────────────────────────────────────────────

/// This agent's session: the terminal it runs in and the project that owns it.
struct Session {
    terminal_id: String,
    project: okena_core::api::ApiProject,
}

fn current_session() -> Result<Session, String> {
    let terminal_id = session_terminal(std::env::var("OKENA_TERMINAL_ID").ok())?;

    let token = super::ensure_token()?;
    let state = super::commands::fetch_state(&token)?;
    let project = state
        .projects
        .into_iter()
        .find(|p| {
            p.layout
                .as_ref()
                .is_some_and(|l| layout_has_terminal(l, &terminal_id))
                || p.terminal_names.contains_key(&terminal_id)
        })
        .ok_or_else(|| format!("No okena project owns terminal {terminal_id}."))?;

    Ok(Session {
        terminal_id,
        project,
    })
}

/// The terminal this server speaks for, from `$OKENA_TERMINAL_ID`. Without one
/// there is no session, and every tool that writes refuses.
fn session_terminal(env: Option<String>) -> Result<String, String> {
    env.filter(|id| !id.trim().is_empty()).ok_or_else(|| {
        "Not running inside an okena terminal (OKENA_TERMINAL_ID is unset), so there is \
         no session to act on."
            .to_string()
    })
}

/// Whether `layout` contains `terminal_id`.
fn layout_has_terminal(layout: &okena_core::api::ApiLayoutNode, terminal_id: &str) -> bool {
    use okena_core::api::ApiLayoutNode as N;
    match layout {
        N::Terminal {
            terminal_id: id, ..
        } => id.as_deref() == Some(terminal_id),
        N::Split { children, .. } | N::Tabs { children, .. } => {
            children.iter().any(|c| layout_has_terminal(c, terminal_id))
        }
    }
}

// ─── Tools ───────────────────────────────────────────────────────────────────

fn whoami() -> Result<Value, String> {
    let session = current_session()?;
    Ok(json!({
        "terminal_id": session.terminal_id,
        "project_id": session.project.id,
        "project_name": session.project.name,
        "project_path": session.project.path,
        "branch": session.project.git_status.as_ref().and_then(|g| g.branch.clone()),
        "task": session.project.task_ref,
        "also_tasks": session.project.also_tasks,
        "agent": session.project.agent,
    }))
}

fn list_projects() -> Result<Value, String> {
    let token = super::ensure_token()?;
    let state = super::commands::fetch_state(&token)?;
    let projects: Vec<Value> = state
        .projects
        .iter()
        .map(|p| {
            json!({
                "id": p.id,
                "name": p.name,
                "path": p.path,
                "branch": p.git_status.as_ref().and_then(|g| g.branch.clone()),
                "is_worktree": p.worktree_info.is_some(),
                "worktree_ids": p.worktree_ids,
                "task": p.task_ref,
                "also_tasks": p.also_tasks,
            })
        })
        .collect();
    Ok(json!({ "projects": projects }))
}

fn report_status(args: &Value) -> Result<Value, String> {
    let status = args
        .get("status")
        .and_then(|s| s.as_str())
        .ok_or("`status` is required")?
        .to_string();
    let session = current_session()?;
    let token = super::ensure_token()?;

    super::api_action(
        &token,
        &json!({
            "action": "agent_report_status",
            "project_id": session.project.id,
            "status": status,
            "state": args.get("state"),
            "question": args.get("question"),
            "suggestions": args.get("suggestions").cloned().unwrap_or_else(|| json!([])),
        })
        .to_string(),
    )?;
    Ok(json!({ "ok": true, "project_id": session.project.id }))
}

/// The task this session was started for, or an explicit one.
///
/// Defaulting to the session's own task is what makes the breakdown tools
/// usable without the agent having to discover an id first — it is already
/// working on exactly one task.
fn target_task(args: &Value) -> Result<(String, String), String> {
    if let Some(explicit) = args
        .get("parent")
        .or_else(|| args.get("task"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let provider = session_provider(&current_session()?)?;
        return Ok((provider, explicit.to_string()));
    }
    let session = current_session()?;
    let task = session.project.task_ref.as_ref().ok_or(
        "this session is not linked to a task — pass `parent` to say which task to work under",
    )?;
    Ok((task.id.provider.clone(), task.id.external_id.clone()))
}

/// The provider a session's tasks live on: its own task's, or else the one the
/// harness is set to.
fn session_provider(session: &Session) -> Result<String, String> {
    match session.project.task_ref.as_ref() {
        Some(t) => Ok(t.id.provider.clone()),
        None => active_provider(),
    }
}

/// A non-blank string argument.
fn str_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// Run a daemon action and hand back its result as JSON, so a tool returns
/// structure the agent can read rather than a string of escaped JSON.
fn action(body: &Value) -> Result<Value, String> {
    let token = super::ensure_token()?;
    let response = super::api_action(&token, &body.to_string())?;
    Ok(serde_json::from_str(&response).unwrap_or(Value::String(response)))
}

/// The task provider the harness is set to, from the daemon's settings.
fn active_provider() -> Result<String, String> {
    let token = super::ensure_token()?;
    let response = super::api_action(&token, &json!({ "action": "get_settings" }).to_string())?;
    let settings: Value =
        serde_json::from_str(&response).map_err(|e| format!("could not read settings: {e}"))?;
    Ok(provider_from_settings(&settings))
}

/// `harness.task_provider`, or Linear for a daemon that predates the setting —
/// Linear was the only provider such a daemon had.
fn provider_from_settings(settings: &Value) -> String {
    settings
        .get("harness")
        .and_then(|h| h.get("task_provider"))
        .and_then(|p| p.as_str())
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .unwrap_or("linear")
        .to_string()
}

fn list_subtasks(args: &Value) -> Result<Value, String> {
    let (provider, task) = target_task(args)?;
    let token = super::ensure_token()?;
    let response = super::api_action(
        &token,
        &json!({
            "action": "task_children",
            "provider": provider,
            "task_external_id": task,
        })
        .to_string(),
    )?;
    Ok(json!({ "parent": task, "children": response }))
}

/// Start one agent on a group of sub-tasks.
///
/// The group is the point: a coordinating agent decides that two tasks cannot
/// be tested apart and hands both to one agent. okena gives each of them
/// worktrees of its own on its own branch, names the session after the first,
/// links it to all of them, and tells the agent which worktree is whose.
fn start_work(args: &Value) -> Result<Value, String> {
    let tasks: Vec<String> = args
        .get("tasks")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let (primary, rest) = tasks
        .split_first()
        .ok_or("`tasks` needs at least one sub-task key")?;

    let session = current_session()?;
    let provider = session
        .project
        .task_ref
        .as_ref()
        .map(|t| t.id.provider.clone())
        .unwrap_or_else(|| "linear".to_string());
    // The repos this session was given. A sub-agent works in the same ones —
    // it is a share of this task, not a different project.
    let project_ids = sibling_repo_ids(&session)?;

    // Only what the agent itself wrote. That the group also covers other
    // sub-tasks is okena's to say, in its `group-note` partial.
    let note = args
        .get("note")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .unwrap_or_default()
        .to_string();

    let token = super::ensure_token()?;
    let response = super::api_action(
        &token,
        &json!({
            "action": "task_start_work",
            "provider": provider,
            "task_external_id": primary,
            "project_ids": project_ids,
            "agent_command": args.get("agent").and_then(|a| a.as_str()),
            "note": (!note.is_empty()).then_some(note),
            "also": rest,
        })
        .to_string(),
    )?;
    Ok(json!({ "started": response, "task": primary, "also": rest }))
}

/// The repositories the current session works in.
///
/// Read off the session's own workspace rather than asked for, so a
/// coordinating agent does not have to know okena's project ids to start a
/// sub-agent beside itself.
fn sibling_repo_ids(session: &Session) -> Result<Vec<String>, String> {
    let token = super::ensure_token()?;
    let state = super::commands::fetch_state(&token)?;
    repo_ids_of(&session.project, &state.projects)
}

/// Which repos `session` works in, among `projects`.
///
/// The repos it was given, when it keeps them: a coordinator over picked tasks
/// has no worktree to say. Otherwise the repos its worktrees were cut from —
/// the worktrees of every task it covers, since each has its own.
fn repo_ids_of(
    session: &okena_core::api::ApiProject,
    projects: &[okena_core::api::ApiProject],
) -> Result<Vec<String>, String> {
    if !session.repo_ids.is_empty() {
        return Ok(session.repo_ids.clone());
    }
    if session.task_ref.is_none() {
        return Err(
            "this session is not linked to a task, so okena cannot tell which repos a \
             sub-agent should work in"
                .to_string(),
        );
    }
    let covered: Vec<&str> = session
        .task_ref
        .iter()
        .chain(&session.also_tasks)
        .map(|t| t.id.external_id.as_str())
        .collect();
    let mut unique: Vec<String> = Vec::new();
    for p in projects {
        let covers = p
            .task_ref
            .as_ref()
            .is_some_and(|t| covered.contains(&t.id.external_id.as_str()));
        if let Some(w) = p.worktree_info.as_ref().filter(|_| covers)
            && !unique.contains(&w.parent_project_id)
        {
            unique.push(w.parent_project_id.clone());
        }
    }
    if unique.is_empty() {
        return Err(
            "this session has no worktrees, so okena cannot tell which repos a \
             sub-agent should work in"
                .to_string(),
        );
    }
    Ok(unique)
}

#[cfg(test)]
mod repo_ids_tests {
    use super::repo_ids_of;
    use okena_core::api::ApiProject;
    use serde_json::json;

    fn task(id: &str) -> serde_json::Value {
        json!({
            "id": { "provider": "linear", "external_id": id },
            "display_key": id.to_uppercase(), "title": "t", "url": "http://x",
        })
    }

    fn project(extra: serde_json::Value) -> ApiProject {
        let mut base = json!({
            "id": "p", "name": "p", "path": "/p", "show_in_overview": true,
            "layout": null, "terminal_names": {},
        });
        for (k, v) in extra.as_object().expect("an object") {
            base[k] = v.clone();
        }
        serde_json::from_value(base).expect("an ApiProject")
    }

    fn worktree(id: &str, repo: &str, task_id: &str) -> ApiProject {
        project(json!({
            "id": id,
            "worktree_info": { "parent_project_id": repo, "branch_name": "b" },
            "task_ref": task(task_id),
        }))
    }

    #[test]
    fn a_coordinator_with_no_worktree_uses_the_repos_it_was_given() {
        let coordinator = project(json!({
            "id": "c1", "task_ref": task("u1"), "repo_ids": ["okena", "web"],
        }));
        assert_eq!(
            repo_ids_of(&coordinator, std::slice::from_ref(&coordinator)).unwrap(),
            ["okena", "web"]
        );
    }

    #[test]
    fn a_session_on_several_tasks_finds_repos_through_every_tasks_worktrees() {
        let session = project(json!({
            "id": "s1", "task_ref": task("u1"), "also_tasks": [task("u2")],
        }));
        let projects = [
            session.clone(),
            worktree("wt1", "okena", "u1"),
            worktree("wt2", "web", "u2"),
            worktree("wt3", "okena", "u2"),
            worktree("other", "infra", "u9"),
        ];
        assert_eq!(repo_ids_of(&session, &projects).unwrap(), ["okena", "web"]);
    }

    #[test]
    fn a_session_with_nothing_to_go_on_says_so() {
        let unlinked = project(json!({ "id": "s1" }));
        assert!(
            repo_ids_of(&unlinked, &[])
                .unwrap_err()
                .contains("not linked")
        );
        let bare = project(json!({ "id": "s1", "task_ref": task("u1") }));
        assert!(repo_ids_of(&bare, &[]).unwrap_err().contains("no worktrees"));
    }
}

fn list_containers() -> Result<Value, String> {
    let provider = session_provider(&current_session()?)?;
    let containers = fetch_containers(&provider)?;
    Ok(json!({ "provider": provider, "containers": containers }))
}

fn fetch_containers(provider: &str) -> Result<Vec<Value>, String> {
    let response = action(&json!({ "action": "task_containers", "provider": provider }))?;
    Ok(response
        .get("containers")
        .and_then(|c| c.as_array())
        .cloned()
        .unwrap_or_default())
}

/// Which container a new top-level task goes in, or the choice to put back
/// to the agent.
///
/// Matched against whatever the agent is likely to have been told: the id, the
/// key (`QBL`), the name, or the `Name (KEY)` form the task-create brief uses.
/// With nothing asked for, a single container is not a choice. Otherwise the
/// options go back as a normal result — the agent has to decide, not recover.
fn pick_container(asked: Option<&str>, containers: &[Value]) -> Result<String, Value> {
    let field = |c: &Value, k: &str| c.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let Some(asked) = asked else {
        return match containers {
            [only] => Ok(field(only, "id")),
            _ => Err(json!({
                "needs_choice": "Which team or project should this go in? Call again with one of \
                                 these as `container`.",
                "options": containers,
            })),
        };
    };
    let found = containers.iter().find(|c| {
        let (id, name, key) = (field(c, "id"), field(c, "name"), field(c, "key"));
        id == asked
            || name.eq_ignore_ascii_case(asked)
            || (!key.is_empty()
                && (key.eq_ignore_ascii_case(asked)
                    || format!("{name} ({key})").eq_ignore_ascii_case(asked)))
    });
    match found {
        Some(c) => Ok(field(c, "id")),
        None => Err(json!({
            "needs_choice": format!(
                "No team or project matches `{asked}`. Call again with one of these as `container`."
            ),
            "options": containers,
        })),
    }
}

/// File a task: top-level in a team or project, or a child of another.
fn create_task(args: &Value) -> Result<Value, String> {
    file_task(args, false)
}

/// `okena_create_task` with `parent` defaulting to this session's task, as
/// briefs written before top-level tasks existed expect.
fn create_subtask(args: &Value) -> Result<Value, String> {
    file_task(args, true)
}

fn file_task(args: &Value, parent_defaults_to_session: bool) -> Result<Value, String> {
    let title = str_arg(args, "title")
        .ok_or("`title` is required")?
        .to_string();
    let session = current_session()?;
    let provider = session_provider(&session)?;
    let parent = match str_arg(args, "parent") {
        Some(parent) => Some(parent.to_string()),
        None if parent_defaults_to_session => Some(
            session
                .project
                .task_ref
                .as_ref()
                .map(|t| t.id.external_id.clone())
                .ok_or(
                    "this session is not linked to a task — pass `parent` to say which task \
                     to work under",
                )?,
        ),
        None => None,
    };

    // A child goes where its parent is; only a top-level task needs a place.
    let container = if parent.is_some() {
        None
    } else {
        let containers = fetch_containers(&provider)?;
        if containers.is_empty() {
            return Err(format!(
                "{provider} lists no teams or projects you can file a task in"
            ));
        }
        match pick_container(str_arg(args, "container"), &containers) {
            Ok(id) => Some(id),
            Err(choice) => return Ok(choice),
        }
    };

    let created = action(&task_create_body(
        &provider,
        &title,
        args,
        parent.as_deref(),
        container.as_deref(),
        &session.project.id,
    ))?;
    // A choice the provider needs made is the answer, not a created task.
    if created.get("needs_choice").is_some() {
        return Ok(created);
    }
    // The alias answers in the shape briefs were written against: the created
    // task as a JSON string.
    let created = if parent_defaults_to_session {
        Value::String(created.to_string())
    } else {
        created
    };
    Ok(json!({ "created": created, "parent": parent }))
}

/// The daemon action that files a task. `project_id` is the filing session, so
/// okena records the task as something that session produced.
fn task_create_body(
    provider: &str,
    title: &str,
    args: &Value,
    parent: Option<&str>,
    container: Option<&str>,
    project_id: &str,
) -> Value {
    json!({
        "action": "task_create",
        "provider": provider,
        "title": title,
        "description": args.get("description").and_then(|d| d.as_str()).unwrap_or(""),
        "kind": args.get("kind").and_then(|k| k.as_str()).unwrap_or("task"),
        "parent_external_id": parent,
        "container_id": container,
        "project_id": project_id,
    })
}

fn get_task(args: &Value) -> Result<Value, String> {
    let (provider, task) = target_task(args)?;
    let found = action(&json!({
        "action": "task_get",
        "provider": provider,
        "task_external_id": task,
    }))?;
    Ok(json!({ "task": found }))
}

fn update_task(args: &Value) -> Result<Value, String> {
    let title = args.get("title").and_then(|t| t.as_str());
    let description = args.get("description").and_then(|d| d.as_str());
    if title.is_none() && description.is_none() {
        return Err("pass `title`, `description` or both — there is nothing to change".into());
    }
    let (provider, task) = target_task(args)?;
    let updated = action(&json!({
        "action": "task_update",
        "provider": provider,
        "task_external_id": task,
        "title": title,
        "description": description,
    }))?;
    Ok(json!({ "task": updated }))
}

fn set_task_state(args: &Value) -> Result<Value, String> {
    let state = parse_state(str_arg(args, "state").ok_or("`state` is required")?)?;
    let (provider, task) = target_task(args)?;
    let moved = action(&json!({
        "action": "task_set_state",
        "provider": provider,
        "task_external_id": task,
        "state": state,
    }))?;
    Ok(json!({ "task": moved }))
}

fn comment_task(args: &Value) -> Result<Value, String> {
    let body = str_arg(args, "body")
        .ok_or("`body` is required")?
        .to_string();
    let (provider, task) = target_task(args)?;
    action(&json!({
        "action": "task_comment",
        "provider": provider,
        "task_external_id": task,
        "body": body,
    }))
}

/// The states an agent can move a task to: okena's normalized categories, since
/// a team's own column names are its business.
const TASK_STATES: &[&str] = &[
    "backlog",
    "todo",
    "in_progress",
    "in_review",
    "done",
    "canceled",
];

/// A state as an agent wrote it — `In Progress`, `in-review` and `cancelled`
/// all mean what they say.
fn parse_state(raw: &str) -> Result<&'static str, String> {
    let mut normalized = raw.trim().to_ascii_lowercase().replace([' ', '-'], "_");
    if normalized == "cancelled" {
        normalized = "canceled".into();
    }
    TASK_STATES
        .iter()
        .copied()
        .find(|s| *s == normalized)
        .ok_or_else(|| {
            format!(
                "unknown state `{raw}` — use one of: {}",
                TASK_STATES.join(", ")
            )
        })
}

/// Whether a daemon refused an action for carrying `field`, which it predates.
fn rejects_field(error: &str, field: &str) -> bool {
    error.contains(&format!("unknown field `{field}`"))
}

fn register_asset(args: &Value) -> Result<Value, String> {
    let kind = args
        .get("kind")
        .and_then(|k| k.as_str())
        .ok_or("`kind` is required")?
        .to_string();
    let title = args
        .get("title")
        .and_then(|t| t.as_str())
        .ok_or("`title` is required")?
        .to_string();
    let session = current_session()?;
    let token = super::ensure_token()?;

    let mut body = json!({
        "action": "agent_register_asset",
        "project_id": session.project.id,
        "kind": kind,
        "title": title,
    });
    for key in ["url", "project", "branch"] {
        if let Some(v) = args
            .get(key)
            .and_then(|v| v.as_str())
            .filter(|v| !v.is_empty())
        {
            body[key] = json!(v);
        }
    }

    let response = match super::api_action(&token, &body.to_string()) {
        // A daemon from before `branch` refuses the whole action over it. One
        // still running after a self-update should not lose the asset.
        Err(error) if body.get("branch").is_some() && rejects_field(&error, "branch") => {
            if let Some(fields) = body.as_object_mut() {
                fields.remove("branch");
            }
            super::api_action(&token, &body.to_string())?
        }
        other => other?,
    };
    Ok(json!({ "ok": true, "response": response }))
}

// ─── Verification runs ───────────────────────────────────────────────────────
//
// Arguments are checked before the session is looked up, so a malformed call
// is told what is wrong without a round trip. The session still gates every
// write: none of these posts anything without one.

/// A test-run action for the session: `fields` plus who is reporting.
fn run_body(action: &str, session: (&str, &str), fields: Value) -> Value {
    let mut body = fields;
    body["action"] = json!(action);
    body["project_id"] = json!(session.0);
    body["terminal_id"] = json!(session.1);
    body
}

/// A step number as an agent sent it: a whole number from 1.
fn step_arg(args: &Value) -> Result<u64, String> {
    args.get("step")
        .and_then(|v| v.as_u64())
        .filter(|n| *n >= 1)
        .ok_or_else(|| "`step` is required: the step's number in the plan, from 1".to_string())
}

fn plan_fields(args: &Value) -> Result<Value, String> {
    let steps = args
        .get("steps")
        .and_then(|v| v.as_array())
        .filter(|a| !a.is_empty())
        .ok_or("`steps` is required: the plan, as a list of { title, proves }")?;
    let steps = steps
        .iter()
        .enumerate()
        .map(|(i, step)| {
            let title =
                str_arg(step, "title").ok_or_else(|| format!("step {} needs a `title`", i + 1))?;
            Ok(json!({ "title": title, "proves": str_arg(step, "proves").unwrap_or("") }))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(json!({ "steps": steps }))
}

fn step_start_fields(args: &Value) -> Result<Value, String> {
    Ok(json!({ "step": step_arg(args)? }))
}

fn step_result_fields(args: &Value) -> Result<Value, String> {
    let step = step_arg(args)?;
    let outcome = str_arg(args, "outcome")
        .map(str::to_ascii_lowercase)
        .filter(|o| ["passed", "failed", "skipped"].contains(&o.as_str()))
        .ok_or("`outcome` is required: passed, failed or skipped")?;
    let reason = str_arg(args, "reason")
        .ok_or("`reason` is required: one line on why — for a failure, what went wrong")?;
    let mut evidence = json!({});
    for key in ["log_tail", "url", "screenshot_path"] {
        if let Some(value) = str_arg(args, key) {
            evidence[key] = json!(value);
        }
    }
    Ok(json!({ "step": step, "outcome": outcome, "reason": reason, "evidence": evidence }))
}

fn run_finish_fields(args: &Value) -> Result<Value, String> {
    let verdict = str_arg(args, "verdict")
        .map(str::to_ascii_lowercase)
        .filter(|v| ["passed", "failed", "inconclusive"].contains(&v.as_str()))
        .ok_or("`verdict` is required: passed, failed or inconclusive")?;
    Ok(json!({ "verdict": verdict, "summary": str_arg(args, "summary") }))
}

/// Post a test-run action for this session.
fn report_run(action_name: &str, fields: Value) -> Result<Value, String> {
    let session = current_session()?;
    let reply = action(&run_body(
        action_name,
        (&session.project.id, &session.terminal_id),
        fields,
    ))?;
    Ok(json!({ "ok": true, "result": reply }))
}

fn test_plan(args: &Value) -> Result<Value, String> {
    report_run("agent_test_plan", plan_fields(args)?)
}

fn test_step_start(args: &Value) -> Result<Value, String> {
    report_run("agent_test_step_start", step_start_fields(args)?)
}

fn test_step_result(args: &Value) -> Result<Value, String> {
    report_run("agent_test_step_result", step_result_fields(args)?)
}

fn test_run_finish(args: &Value) -> Result<Value, String> {
    report_run("agent_test_run_finish", run_finish_fields(args)?)
}

#[cfg(test)]
mod tests {
    use super::{
        INVALID_PARAMS, METHOD_NOT_FOUND, PROTOCOL_VERSION, dispatch, layout_has_terminal,
        tool_definitions, tool_error, tool_result,
    };
    use okena_core::api::ApiLayoutNode;
    use okena_core::shell::ShellType;
    use serde_json::json;

    fn terminal(id: &str) -> ApiLayoutNode {
        ApiLayoutNode::Terminal {
            terminal_id: Some(id.to_string()),
            minimized: false,
            detached: false,
            shell_type: ShellType::Default,
            cols: None,
            rows: None,
        }
    }

    #[test]
    fn the_active_provider_comes_from_harness_settings() {
        use super::provider_from_settings;
        assert_eq!(
            provider_from_settings(&json!({ "harness": { "task_provider": "azure_devops" } })),
            "azure_devops"
        );
        // A daemon from before the setting only ever had Linear.
        assert_eq!(provider_from_settings(&json!({ "harness": {} })), "linear");
        assert_eq!(provider_from_settings(&json!({})), "linear");
    }

    #[test]
    fn a_container_is_found_by_whatever_the_agent_was_told() {
        use super::pick_container;
        let teams = [
            json!({ "id": "t1", "name": "Qblok", "key": "QBL" }),
            json!({ "id": "t2", "name": "Platform", "key": "PLT" }),
        ];
        for asked in ["t1", "QBL", "qbl", "Qblok", "Qblok (QBL)"] {
            assert_eq!(
                pick_container(Some(asked), &teams),
                Ok("t1".to_string()),
                "{asked}"
            );
        }
    }

    #[test]
    fn an_open_choice_comes_back_as_options_not_an_error() {
        use super::pick_container;
        let teams = [
            json!({ "id": "t1", "name": "Qblok", "key": "QBL" }),
            json!({ "id": "t2", "name": "Platform", "key": "PLT" }),
        ];
        for asked in [None, Some("Nope")] {
            let choice = pick_container(asked, &teams).expect_err("must ask");
            assert!(choice["needs_choice"].as_str().is_some(), "{choice}");
            assert_eq!(choice["options"].as_array().map(Vec::len), Some(2));
        }
        // One team is not a choice.
        assert_eq!(pick_container(None, &teams[..1]), Ok("t1".to_string()));
    }

    #[test]
    fn states_are_read_forgivingly_and_checked_before_any_call() {
        use super::parse_state;
        assert_eq!(parse_state("In Progress"), Ok("in_progress"));
        assert_eq!(parse_state("in-review"), Ok("in_review"));
        assert_eq!(parse_state("cancelled"), Ok("canceled"));
        assert!(parse_state("shipped").is_err());
    }

    #[test]
    fn a_filed_task_names_the_session_that_filed_it() {
        let body = super::task_create_body(
            "linear",
            "Split payments",
            &json!({ "kind": "story" }),
            Some("QBL-9"),
            None,
            "session-1",
        );
        assert_eq!(body["project_id"], "session-1");
        assert_eq!(body["parent_external_id"], "QBL-9");
        assert_eq!(body["kind"], "story");
        // And the daemon reads it as the action it names.
        let parsed: okena_core::api::ActionRequest =
            serde_json::from_value(body).expect("a TaskCreate the daemon accepts");
        assert!(matches!(
            parsed,
            okena_core::api::ActionRequest::TaskCreate { project_id: Some(ref p), .. }
                if p == "session-1"
        ));
    }

    #[test]
    fn outside_an_okena_terminal_there_is_no_session_to_write_for() {
        use super::session_terminal;
        assert!(session_terminal(None).is_err());
        assert!(session_terminal(Some("  ".into())).is_err());
        assert_eq!(session_terminal(Some("t1".into())), Ok("t1".to_string()));
    }

    /// Parse a tool's body as the daemon will.
    fn as_action(action: &str, fields: serde_json::Value) -> okena_core::api::ActionRequest {
        serde_json::from_value(super::run_body(action, ("session-1", "term-1"), fields))
            .expect("an action the daemon accepts")
    }

    #[test]
    fn a_plan_crosses_to_the_daemon_as_its_steps() {
        use okena_core::api::ActionRequest;
        let fields = super::plan_fields(&json!({ "steps": [
            { "title": "Build", "proves": "it compiles" },
            { "title": "Run e2e" },
        ]}))
        .expect("fields");
        match as_action("agent_test_plan", fields) {
            ActionRequest::AgentTestPlan {
                project_id,
                terminal_id,
                steps,
            } => {
                assert_eq!(
                    (project_id.as_str(), terminal_id.as_str()),
                    ("session-1", "term-1")
                );
                assert_eq!(steps.len(), 2);
                assert_eq!(steps[0].proves, "it compiles");
                assert_eq!(steps[1].proves, "");
            }
            other => panic!("expected a plan, got {other:?}"),
        }
        assert!(super::plan_fields(&json!({ "steps": [] })).is_err());
        let untitled = super::plan_fields(&json!({ "steps": [{ "proves": "x" }] }));
        assert!(untitled.is_err_and(|e| e.contains("step 1")));
    }

    #[test]
    fn a_step_result_carries_its_evidence_to_the_daemon() {
        use okena_core::api::{ActionRequest, VerificationStepState};
        let fields = super::step_result_fields(&json!({
            "step": 2,
            "outcome": "Failed",
            "reason": "login redirects forever",
            "log_tail": "302 /login",
            "screenshot_path": "/tmp/login.png",
            "url": "",
        }))
        .expect("fields");
        match as_action("agent_test_step_result", fields) {
            ActionRequest::AgentTestStepResult {
                step,
                outcome,
                reason,
                evidence,
                ..
            } => {
                assert_eq!(step, 2);
                assert_eq!(outcome, VerificationStepState::Failed);
                assert_eq!(reason, "login redirects forever");
                assert_eq!(evidence.log_tail.as_deref(), Some("302 /login"));
                assert_eq!(evidence.screenshot_path.as_deref(), Some("/tmp/login.png"));
                assert_eq!(evidence.url, None, "a blank url is no evidence");
            }
            other => panic!("expected a step result, got {other:?}"),
        }
    }

    #[test]
    fn malformed_run_reports_are_refused_before_any_call() {
        use super::{run_finish_fields, step_result_fields, step_start_fields};
        assert!(step_start_fields(&json!({ "step": 0 })).is_err());
        assert!(step_start_fields(&json!({ "step": "1" })).is_err());
        assert!(
            step_result_fields(&json!({ "step": 1, "outcome": "running", "reason": "x" })).is_err()
        );
        assert!(step_result_fields(&json!({ "step": 1, "outcome": "passed" })).is_err());
        assert!(run_finish_fields(&json!({ "verdict": "shipped" })).is_err());
    }

    #[test]
    fn a_start_and_a_finish_parse_as_the_daemon_reads_them() {
        use okena_core::api::{ActionRequest, VerificationVerdict};
        let start = super::step_start_fields(&json!({ "step": 1 })).expect("fields");
        assert!(matches!(
            as_action("agent_test_step_start", start),
            ActionRequest::AgentTestStepStart { step: 1, .. }
        ));
        let finish =
            super::run_finish_fields(&json!({ "verdict": "inconclusive" })).expect("fields");
        assert!(matches!(
            as_action("agent_test_run_finish", finish),
            ActionRequest::AgentTestRunFinish {
                verdict: VerificationVerdict::Inconclusive,
                summary: None,
                ..
            }
        ));
    }

    #[test]
    fn initialize_reports_protocol_and_tools_capability() {
        let r = dispatch("initialize", &json!({})).expect("initialize must succeed");
        assert_eq!(r["protocolVersion"], PROTOCOL_VERSION);
        assert!(r["capabilities"]["tools"].is_object());
        assert_eq!(r["serverInfo"]["name"], "okena");
    }

    #[test]
    fn unknown_method_is_a_protocol_error() {
        let e = dispatch("nope", &json!({})).expect_err("must reject");
        assert_eq!(e["code"], METHOD_NOT_FOUND);
    }

    #[test]
    fn tools_list_advertises_every_tool_with_a_schema() {
        let r = dispatch("tools/list", &json!({})).expect("tools/list must succeed");
        let tools = r["tools"].as_array().expect("array");

        // The exact set, not a count: a bare number says nothing about which
        // tool went missing, and adding one should be a deliberate edit here.
        let mut names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
        names.sort_unstable();
        assert_eq!(
            names,
            [
                "okena_comment_task",
                "okena_create_subtask",
                "okena_create_task",
                "okena_get_task",
                "okena_list_containers",
                "okena_list_projects",
                "okena_list_subtasks",
                "okena_register_asset",
                "okena_report_status",
                "okena_set_task_state",
                "okena_start_work",
                "okena_test_plan",
                "okena_test_run_finish",
                "okena_test_step_result",
                "okena_test_step_start",
                "okena_update_task",
                "okena_whoami",
            ]
        );
        for t in tools {
            assert!(t["name"].as_str().is_some_and(|n| n.starts_with("okena_")));
            assert!(
                t["description"].as_str().is_some_and(|d| !d.is_empty()),
                "every tool needs a description — it is how the model chooses"
            );
            assert_eq!(t["inputSchema"]["type"], "object");
        }
    }

    #[test]
    fn tool_call_without_a_name_is_invalid_params() {
        let e = dispatch("tools/call", &json!({})).expect_err("must reject");
        assert_eq!(e["code"], INVALID_PARAMS);
    }

    #[test]
    fn unknown_tool_is_rejected() {
        let e = dispatch("tools/call", &json!({ "name": "okena_nope" })).expect_err("must reject");
        assert_eq!(e["code"], METHOD_NOT_FOUND);
    }

    #[test]
    fn tool_failures_are_tool_errors_not_protocol_errors() {
        // A model never sees a JSON-RPC error, so a failing tool must come back
        // as a normal result carrying isError.
        let r = dispatch("tools/call", &json!({ "name": "okena_report_status" }))
            .expect("tool call itself succeeds");
        assert_eq!(r["isError"], true);
        assert!(
            r["content"][0]["text"]
                .as_str()
                .is_some_and(|t| t.contains("status")),
            "the reason must be readable by the model"
        );
    }

    #[test]
    fn results_render_json_as_text_content() {
        let r = tool_result(&json!({ "a": 1 }));
        let text = r["content"][0]["text"].as_str().expect("text");
        assert!(text.contains("\"a\""));
        assert!(r.get("isError").is_none());
    }

    #[test]
    fn tool_error_marks_itself() {
        let e = tool_error("boom");
        assert_eq!(e["isError"], true);
        assert_eq!(e["content"][0]["text"], "boom");
    }

    #[test]
    fn finds_a_terminal_nested_in_splits_and_tabs() {
        let layout = ApiLayoutNode::Split {
            direction: okena_core::types::SplitDirection::Horizontal,
            sizes: vec![0.5, 0.5],
            children: vec![
                terminal("a"),
                ApiLayoutNode::Tabs {
                    children: vec![terminal("b"), terminal("target")],
                    active_tab: 0,
                },
            ],
        };
        assert!(layout_has_terminal(&layout, "target"));
        assert!(layout_has_terminal(&layout, "a"));
        assert!(!layout_has_terminal(&layout, "missing"));
    }

    #[test]
    fn every_advertised_tool_is_dispatchable() {
        // A tool advertised but not handled would fail only at call time, in an
        // agent's session, with a confusing "unknown tool".
        for t in tool_definitions().as_array().expect("array") {
            let name = t["name"].as_str().expect("name");
            let r = dispatch("tools/call", &json!({ "name": name }));
            assert!(
                r.is_ok(),
                "advertised tool {name} was rejected as unknown by dispatch"
            );
        }
    }

    #[test]
    fn only_a_daemon_that_predates_the_field_is_retried_without_it() {
        // What an older daemon's axum body rejection surfaces as.
        let older = "Server returned 422 Unprocessable Entity: Failed to deserialize the JSON \
                     body into the target type: unknown field `branch`, expected one of `url`";
        assert!(super::rejects_field(older, "branch"));
        assert!(!super::rejects_field(older, "project"));
        assert!(!super::rejects_field("project not found: s1", "branch"));
    }
}
