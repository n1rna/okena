//! The wasmtime host against a real component: the `probe` fixture.

mod support;

use std::sync::{Arc, Mutex};

use okena_core::extension::{ExtNode, ExtPermissions, Invoker};
use okena_extension_host::exec::SearchPath;
use okena_extension_host::manifest::Manifest;
use okena_extension_host::permissions::Guard;
use okena_extension_host::runtime::{CallError, Environment, HostProject, Instance, Runtime};
use serde_json::{Value, json};

struct Probe {
    instance: Instance,
    refusals: Arc<Mutex<Vec<String>>>,
    _dir: tempfile::TempDir,
}

fn probe_with(permissions: ExtPermissions, config: Value) -> Option<Probe> {
    let wasm = support::fixture_wasm("probe")?;
    let runtime = Runtime::new().expect("runtime");
    let component = runtime.compile(&std::fs::read(wasm).expect("read wasm")).expect("compile");
    let dir = tempfile::tempdir().expect("tempdir");
    let refusals = Arc::new(Mutex::new(Vec::new()));
    let sink = refusals.clone();
    let instance = Instance::new(
        &runtime,
        component,
        Environment {
            extension_id: "probe".into(),
            guard: Guard::new(permissions, SearchPath::from_env()),
            config,
            kv_path: dir.path().join("kv.json"),
            projects: Arc::new(|| {
                vec![HostProject {
                    id: "p1".into(),
                    name: "one".into(),
                    path: "/tmp/one".into(),
                }]
            }),
            refusals: Arc::new(move |m| sink.lock().expect("lock").push(m)),
        },
    )
    .expect("instantiate");
    Some(Probe {
        instance,
        refusals,
        _dir: dir,
    })
}

/// The probe with what its manifest asks for approved.
fn probe(config: Value) -> Option<Probe> {
    let manifest =
        Manifest::load(&support::fixtures_dir().join("probe")).expect("probe manifest");
    let mut permissions = manifest.permissions;
    permissions.commands.push("cat".into());
    probe_with(permissions, config)
}

fn query(p: &mut Probe, id: &str, args: Value) -> Result<Value, CallError> {
    p.instance
        .query(id, &args.to_string())
        .map(|answer| serde_json::from_str(&answer).expect("json answer"))
}

#[test]
fn a_component_built_with_the_sdk_loads_describes_and_refreshes() {
    let Some(mut p) = probe(json!({})) else { return };

    let (actions, queries) = p.instance.describe().expect("describe");
    let ids: Vec<_> = actions.iter().map(|a| a.id.as_str()).collect();
    assert_eq!(ids, ["launch", "wipe", "undeclared"]);
    assert!(actions[1].destructive && actions[1].agent_callable);
    assert_eq!(queries[0].id, "echo");

    let (view, status) = p.instance.refresh().expect("refresh");
    let ExtNode::Stack { children } = &view.nodes[view.root as usize] else {
        panic!("root is the stack: {view:?}")
    };
    assert_eq!(children.len(), 3);
    let ExtNode::Table { table } = &view.nodes[children[0] as usize] else {
        panic!("first child is the table")
    };
    assert_eq!(table.rows[0].cells[0].text, "hello", "the row came from running echo");
    assert_eq!(status.expect("status").label, "1 rows");

    // The instance keeps its state between calls.
    p.instance.refresh().expect("refresh again");
    assert_eq!(query(&mut p, "count", json!({})).expect("count"), json!(2));
}

#[test]
fn an_undeclared_command_is_refused_and_the_refusal_is_reported() {
    let Some(mut p) = probe(json!({})) else { return };
    let err = query(&mut p, "run", json!({ "program": "ls", "args": ["/"] })).expect_err("refused");
    let CallError::Failed(message) = err else { panic!("a refusal is an error, not a crash: {err:?}") };
    assert!(message.contains("running `ls` was refused"), "{message}");
    assert_eq!(p.refusals.lock().expect("lock").len(), 1);

    // An approved command still runs afterwards.
    let out = query(&mut p, "run", json!({ "program": "echo", "args": ["ok"] })).expect("echo");
    assert_eq!(out["stdout"], "ok\n");
    assert_eq!(out["code"], 0);
}

#[test]
fn stdin_reaches_the_command() {
    let Some(mut p) = probe(json!({})) else { return };
    assert_eq!(query(&mut p, "stdin", json!({})).expect("cat"), json!("piped"));
}

#[test]
fn files_are_readable_only_under_approved_paths() {
    let data = tempfile::tempdir().expect("tempdir");
    std::fs::write(data.path().join("a.txt"), "inside").expect("write");
    let outside = tempfile::tempdir().expect("tempdir");
    std::fs::write(outside.path().join("b.txt"), "outside").expect("write");
    let dir = data.path().to_string_lossy().to_string();
    let Some(mut p) = probe(json!({ "dir": dir })) else { return };

    let read = query(&mut p, "read", json!({ "path": format!("{dir}/a.txt") })).expect("read");
    assert_eq!(read, json!("inside"));
    let listed = query(&mut p, "list", json!({ "path": dir })).expect("list");
    assert_eq!(listed, json!(["a.txt"]));

    let secret = outside.path().join("b.txt").to_string_lossy().to_string();
    let err = query(&mut p, "read", json!({ "path": secret })).expect_err("refused");
    assert!(err.message().contains("was refused"), "{err:?}");
    assert_eq!(p.refusals.lock().expect("lock").len(), 1);
}

#[test]
fn a_config_change_reaches_path_permissions_and_the_extension() {
    let data = tempfile::tempdir().expect("tempdir");
    std::fs::write(data.path().join("a.txt"), "x").expect("write");
    let dir = data.path().to_string_lossy().to_string();
    let Some(mut p) = probe(json!({ "dir": "" })) else { return };
    let path = format!("{dir}/a.txt");

    assert!(query(&mut p, "read", json!({ "path": path })).is_err(), "no dir configured yet");
    p.instance.set_config(json!({ "dir": dir }));
    assert_eq!(query(&mut p, "config", json!({})).expect("config")["dir"], json!(dir));
    assert!(query(&mut p, "read", json!({ "path": path })).is_ok());
}

#[test]
fn storage_log_and_projects_work() {
    let Some(mut p) = probe(json!({})) else { return };
    query(&mut p, "kv_set", json!({ "key": "k", "value": "v" })).expect("set");
    assert_eq!(query(&mut p, "kv_get", json!({ "key": "k" })).expect("get"), json!("v"));
    query(&mut p, "log", json!({})).expect("log");
    assert_eq!(query(&mut p, "projects", json!({})).expect("projects"), json!(1));
}

#[test]
fn a_panic_fails_the_call_and_the_next_call_gets_a_fresh_instance() {
    let Some(mut p) = probe(json!({})) else { return };
    p.instance.refresh().expect("refresh");
    let err = query(&mut p, "panic", json!({})).expect_err("trap");
    let CallError::Trapped(message) = &err else { panic!("{err:?}") };
    assert!(message.contains("probe panicked on purpose"), "{message}");

    // The fresh instance starts over, and storage (on disk) survives.
    p.instance.refresh().expect("refresh after the crash");
    assert_eq!(query(&mut p, "count", json!({})).expect("count"), json!(1));
}

#[test]
fn an_endless_loop_is_cut_off_at_the_compute_budget() {
    let Some(mut p) = probe(json!({})) else { return };
    let started = std::time::Instant::now();
    let err = query(&mut p, "spin", json!({})).expect_err("trap");
    assert!(matches!(&err, CallError::Trapped(m) if m.contains("computed for more than")), "{err:?}");
    assert!(started.elapsed() < okena_extension_host::runtime::COMPUTE_BUDGET * 2);
    p.instance.refresh().expect("usable afterwards");
}

#[test]
fn running_out_of_memory_fails_the_call_not_the_host() {
    let Some(mut p) = probe(json!({})) else { return };
    let err = query(&mut p, "hog", json!({})).expect_err("trap");
    assert!(matches!(&err, CallError::Trapped(m) if m.contains("MB of memory")), "{err:?}");
    p.instance.refresh().expect("usable afterwards");
}

#[test]
fn an_agent_action_returns_its_launch() {
    let Some(mut p) = probe(json!({})) else { return };
    let outcome = p
        .instance
        .run_action("launch", &["row-7".into()], &[], Invoker::User)
        .expect("launch");
    let agent = outcome.agent.expect("agent launch");
    assert_eq!(agent.goal, "Look into row-7");
    assert_eq!(agent.item.as_deref(), Some("row-7"));
    assert_eq!(agent.item_label.as_deref(), Some("Row row-7"));

    let err = p
        .instance
        .run_action("launch", &[], &[], Invoker::User)
        .expect_err("needs a row");
    assert!(matches!(err, CallError::Failed(m) if m.contains("needs a row")));
}
