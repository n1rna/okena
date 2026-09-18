//! The example library's extensions, run for real: cli-table through jq,
//! git-tree over a local folder.

mod support;

use std::sync::Arc;

use okena_core::extension::{ExtNode, Invoker};
use okena_extension_host::exec::SearchPath;
use okena_extension_host::manifest::Manifest;
use okena_extension_host::permissions::Guard;
use okena_extension_host::runtime::{Environment, Instance, Runtime};
use serde_json::{Value, json};

fn load(name: &str, config: Value) -> Option<(Instance, tempfile::TempDir)> {
    let wasm = support::example_wasm(name)?;
    let manifest = Manifest::load(&support::example_library_dir().join("extensions").join(name))
        .expect("manifest");
    let runtime = Runtime::new().expect("runtime");
    let component = runtime.compile(&std::fs::read(wasm).expect("read")).expect("compile");
    let data = tempfile::tempdir().expect("tempdir");
    let instance = Instance::new(
        &runtime,
        component,
        Environment {
            extension_id: manifest.id.clone(),
            guard: Guard::new(manifest.permissions.clone(), SearchPath::from_env()),
            config: manifest.effective_config(Some(&config)),
            kv_path: data.path().join("kv.json"),
            projects: Arc::new(Vec::new),
            refusals: Arc::new(|m| panic!("refused: {m}")),
        },
    )
    .expect("instantiate");
    Some((instance, data))
}

fn has_jq() -> bool {
    let found = SearchPath::from_env().resolve("jq").is_some();
    if !found {
        eprintln!("skipping: jq is not installed");
    }
    found
}

#[test]
fn cli_table_groups_jobs_by_tenant_and_its_actions_change_them() {
    if !has_jq() {
        return;
    }
    let Some((mut ext, _data)) = load("cli-table", json!({})) else { return };
    let (actions, _) = ext.describe().expect("describe");
    let delete = actions.iter().find(|a| a.id == "delete").expect("delete");
    assert!(delete.destructive);
    assert!(actions.iter().any(|a| a.id == "investigate_now" && a.agent == Some(okena_core::extension::AgentMode::Start)));

    let (view, status) = ext.refresh().expect("refresh");
    let table = view
        .nodes
        .iter()
        .find_map(|n| match n {
            ExtNode::Table { table } => Some(table),
            _ => None,
        })
        .expect("a table");
    assert_eq!(table.group_by.as_deref(), Some("tenant"));
    assert_eq!(table.rows.len(), 10);
    assert!(table.bulk_actions.contains(&"delete".to_string()));
    assert!(status.expect("status").label.ends_with("stuck"));

    ext.run_action("unblock", &["job-102".into()], &[], Invoker::User).expect("unblock");
    let job: Value = serde_json::from_str(&ext.query("get_job", r#"{"id":"job-102"}"#).expect("get")).expect("json");
    assert_eq!(job["status"], "queued");

    let outcome = ext
        .run_action("delete", &["job-101".into(), "job-102".into()], &[], Invoker::User)
        .expect("delete");
    assert_eq!(outcome.message.as_deref(), Some("Deleted 2 job(s)"));
    let (view, _) = ext.refresh().expect("refresh");
    let rows = view.nodes.iter().find_map(|n| match n {
        ExtNode::Table { table } => Some(table.rows.len()),
        _ => None,
    });
    assert_eq!(rows, Some(8));

    let launch = ext
        .run_action("investigate", &["job-203".into()], &[], Invoker::User)
        .expect("investigate")
        .agent
        .expect("an agent launch");
    assert_eq!(launch.item.as_deref(), Some("job-203"));
    assert!(launch.goal.contains("schema mismatch"), "{}", launch.goal);
}

#[test]
fn git_tree_draws_a_local_folder_as_a_tree_with_charts() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("src/nested")).expect("mkdir");
    std::fs::write(dir.path().join("README.md"), "hello").expect("write");
    std::fs::write(dir.path().join("src/lib.rs"), "fn main() {}").expect("write");
    std::fs::write(dir.path().join("src/nested/a.rs"), "x").expect("write");
    let root = dir.path().to_string_lossy().to_string();
    let Some((mut ext, _data)) = load("git-tree", json!({ "source": "local", "local_path": root })) else {
        return;
    };
    let (view, status) = ext.refresh().expect("refresh");
    let tree = view
        .nodes
        .iter()
        .find_map(|n| match n {
            ExtNode::Tree { tree } => Some(tree),
            _ => None,
        })
        .expect("a tree");
    assert!(tree.show_detail);
    let labels: Vec<&str> = tree.roots.iter().map(|&r| tree.items[r as usize].label.as_str()).collect();
    assert_eq!(labels, vec!["src", "README.md"]);
    let src = &tree.items[tree.roots[0] as usize];
    assert_eq!(src.children.len(), 2, "lib.rs and nested/");
    assert!(view.nodes.iter().any(|n| matches!(n, ExtNode::BarChart { .. })));
    assert!(view.nodes.iter().any(|n| matches!(n, ExtNode::LineChart { .. })));
    assert_eq!(status.expect("status").label, "3 files");
}
