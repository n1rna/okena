//! Which brief each launch flow starts with, as the launchers name it.
//!
//! Every [`AgentLauncher`](okena_ui::agent_launcher::AgentLauncher) that
//! briefs its agent shows the template's name and the model it runs on. Where
//! a template comes from — a knowledge root or okena's built-in — is resolved
//! by the daemon, so this asks it (`PromptRender`) and keeps the answer per
//! connection and flow.
//!
//! Asked for while rendering: an answer older than [`STALE_AFTER`] is fetched
//! again in the background and the old one shown meanwhile, so a template
//! edited in knowledge shows the next time a launcher is looked at.

use gpui::*;
use okena_core::agent_model::AgentModels;
use okena_core::api::ActionRequest;
use okena_transport::remote_action::RemoteActionClient;
use okena_ui::agent_launcher::LaunchBrief;
use std::time::{Duration, Instant};

/// How long an answer is shown before it is asked for again.
const STALE_AFTER: Duration = Duration::from_secs(5);

struct Entry {
    client: RemoteActionClient,
    flow: &'static str,
    brief: Option<LaunchBrief>,
    asked: Instant,
    in_flight: bool,
}

#[derive(Default)]
struct LaunchBriefs {
    entries: Vec<Entry>,
}

impl Global for LaunchBriefs {}

/// The brief `flow` launches with over `client`: the last answer, or `None`
/// until the first arrives. Asks again when that answer is stale.
pub(crate) fn brief_for(
    client: &RemoteActionClient,
    flow: &'static str,
    cx: &mut App,
) -> Option<LaunchBrief> {
    let briefs = cx.default_global::<LaunchBriefs>();
    let now = Instant::now();
    let index = match briefs
        .entries
        .iter()
        .position(|e| e.flow == flow && e.client.same_connection(client))
    {
        Some(i) => i,
        None => {
            briefs.entries.push(Entry {
                client: client.clone(),
                flow,
                brief: None,
                asked: now,
                in_flight: false,
            });
            briefs.entries.len() - 1
        }
    };
    let entry = &mut briefs.entries[index];
    let due = entry.brief.is_none() || now.duration_since(entry.asked) >= STALE_AFTER;
    let shown = entry.brief.clone();
    if due && !entry.in_flight {
        entry.in_flight = true;
        entry.asked = now;
        fetch(client.clone(), flow, cx);
    }
    shown
}

fn fetch(client: RemoteActionClient, flow: &'static str, cx: &mut App) {
    cx.spawn(async move |cx| {
        let asked = client.clone();
        let result = smol::unblock(move || {
            asked.post_action(ActionRequest::PromptRender {
                flow: flow.to_string(),
                vars: Default::default(),
            })
        })
        .await;
        cx.update(|cx| {
            let fetched = match result {
                Ok(Some(value)) => Some(parse(flow, &value)),
                Ok(None) => None,
                Err(e) => {
                    log::debug!("[launch] brief for {flow} not available: {e}");
                    None
                }
            };
            let briefs = cx.default_global::<LaunchBriefs>();
            let Some(entry) = briefs
                .entries
                .iter_mut()
                .find(|e| e.flow == flow && e.client.same_connection(&client))
            else {
                return;
            };
            entry.in_flight = false;
            // A failed ask keeps what was shown rather than blanking it.
            if let Some(brief) = fetched
                && entry.brief.as_ref() != Some(&brief)
            {
                entry.brief = Some(brief);
                cx.refresh_windows();
            }
        })
    })
    .detach();
}

/// A launcher's brief from the daemon's `PromptRender` answer.
fn parse(flow: &str, value: &serde_json::Value) -> LaunchBrief {
    let name = value
        .get("name")
        .and_then(|v| v.as_str())
        .filter(|n| !n.is_empty())
        .unwrap_or(flow);
    let source = value.get("source").unwrap_or(&serde_json::Value::Null);
    let models: AgentModels = value
        .get("models")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    LaunchBrief {
        name: SharedString::from(name.to_string()),
        source: format!("Briefed by {}", describe_brief_source(flow, source)).into(),
        file: brief_file(source),
        models,
    }
}

/// The knowledge file a brief is, when it is one: okena's built-in templates
/// are not on disk, so there is nothing to open.
fn brief_file(source: &serde_json::Value) -> Option<(SharedString, SharedString)> {
    let root = source.get("root")?.as_str()?;
    let path = source.get("path")?.as_str()?;
    Some((SharedString::from(root.to_string()), SharedString::from(path.to_string())))
}

/// Open a brief's own file in Harness → Knowledge.
pub(crate) fn open_brief(
    broker: &Entity<okena_workspace::request_broker::RequestBroker>,
    root: &SharedString,
    path: &SharedString,
    cx: &mut App,
) {
    let request = okena_workspace::requests::WorkbenchRequest::OpenKnowledgeDoc {
        root_key: root.to_string(),
        path: path.to_string(),
    };
    broker.update(cx, |broker, cx| {
        broker.push_workbench_request(request, cx);
    });
}

/// Say where a launch brief's template comes from.
///
/// `source` is the daemon's `PromptRender` answer: `{"builtin": true}` or
/// `{"root": key, "path": path}`. Named so the user knows what to edit.
pub(crate) fn describe_brief_source(flow: &str, source: &serde_json::Value) -> String {
    match (
        source.get("root").and_then(|v| v.as_str()),
        source.get("path").and_then(|v| v.as_str()),
    ) {
        (Some(root), Some(path)) => format!("`{path}` in {root}"),
        _ => format!("okena's built-in `{flow}` template"),
    }
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn a_render_answer_names_the_template_its_source_and_models() {
        let brief = parse(
            "task-start",
            &serde_json::json!({
                "flow": "task-start",
                "name": "acme-start",
                "source": { "root": "store:acme", "path": "templates/task-start.md" },
                "models": { "model": "sonnet", "models": { "codex": "gpt-5-codex" } },
            }),
        );
        assert_eq!(brief.name.as_ref(), "acme-start");
        assert_eq!(
            brief.source.as_ref(),
            "Briefed by `templates/task-start.md` in store:acme"
        );
        assert_eq!(brief.models.for_agent("claude"), Some("sonnet"));
        assert_eq!(brief.models.for_agent("codex"), Some("gpt-5-codex"));
        // The file the chip opens.
        let (root, path) = brief.file.expect("a template in a root is a file");
        assert_eq!((root.as_ref(), path.as_ref()), ("store:acme", "templates/task-start.md"));
    }

    #[test]
    fn a_built_in_template_is_no_file_to_open() {
        let brief = parse("task-start", &serde_json::json!({ "source": { "builtin": true } }));
        assert_eq!(brief.file, None);
    }

    #[test]
    fn an_older_daemons_answer_still_names_the_flow() {
        // No `name` or `models`: called after its flow, on the CLI's default.
        let brief = parse(
            "break-down",
            &serde_json::json!({ "flow": "break-down", "source": { "builtin": true } }),
        );
        assert_eq!(brief.name.as_ref(), "break-down");
        assert_eq!(
            brief.source.as_ref(),
            "Briefed by okena's built-in `break-down` template"
        );
        assert!(brief.models.is_empty());
    }
}
