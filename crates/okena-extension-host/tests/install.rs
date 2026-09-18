//! Install, update, reload and remove against local git repositories laid
//! out like an extension library: `extensions/<id>/` per extension.

mod support;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use okena_core::extension::{ApiExtension, ExtNode, ExtPermissions, ExtRunState, ExtSource};
use okena_extension_host::exec::SearchPath;
use okena_extension_host::host::{ExtensionHost, HostConfig, HostSettings};
use okena_extension_host::store::Dirs;

fn git(repo: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"])
        .args(args)
        .current_dir(repo)
        .output()
        .expect("git");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A library repo holding the probe fixture at `extensions/probe`.
struct Library {
    dir: tempfile::TempDir,
}

impl Library {
    /// With `prebuilt`, the built component is committed next to the manifest.
    fn new(prebuilt: bool) -> Option<Self> {
        let wasm = support::fixture_wasm("probe")?;
        let dir = tempfile::tempdir().expect("tempdir");
        let lib = Library { dir };
        let ext = lib.ext_dir();
        std::fs::create_dir_all(ext.join("src")).expect("mkdir");
        let fixture = support::fixtures_dir().join("probe");
        for file in ["extension.toml", "src/lib.rs"] {
            std::fs::copy(fixture.join(file), ext.join(file)).expect("copy");
        }
        // The SDK is a path dependency; point it back at this checkout.
        let sdk = Path::new(env!("CARGO_MANIFEST_DIR")).join("../okena-extension-api");
        let cargo = std::fs::read_to_string(fixture.join("Cargo.toml"))
            .expect("read")
            .replace("../../../../okena-extension-api", &sdk.to_string_lossy());
        std::fs::write(ext.join("Cargo.toml"), cargo).expect("write");
        if prebuilt {
            std::fs::copy(wasm, ext.join("extension.wasm")).expect("copy wasm");
        }
        // A library pins its toolchain like okena does; this one pins okena's.
        let toolchain = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../rust-toolchain.toml");
        std::fs::copy(toolchain, lib.path().join("rust-toolchain.toml")).expect("copy toolchain");
        git(lib.path(), &["init", "--quiet", "--initial-branch", "main"]);
        lib.commit("first");
        Some(lib)
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn url(&self) -> String {
        self.path().to_string_lossy().into_owned()
    }

    fn ext_dir(&self) -> PathBuf {
        self.path().join("extensions/probe")
    }

    fn commit(&self, message: &str) -> String {
        git(self.path(), &["add", "-A"]);
        git(self.path(), &["commit", "--quiet", "-m", message]);
        git(self.path(), &["rev-parse", "HEAD"])
    }

    fn edit_manifest(&self, from: &str, to: &str) {
        let path = self.ext_dir().join("extension.toml");
        let text = std::fs::read_to_string(&path).expect("read");
        assert!(text.contains(from), "manifest has no {from:?}");
        std::fs::write(path, text.replace(from, to)).expect("write");
    }

    fn source(&self) -> ExtSource {
        ExtSource::Git {
            url: self.url(),
            git_ref: Some("main".into()),
            path: Some("extensions/probe".into()),
            commit: String::new(),
        }
    }
}

struct Harness {
    host: ExtensionHost,
    profile: tempfile::TempDir,
}

/// `name` gives the harness its own cargo target, so tests that build
/// different sources of the same crate never swap each other's output.
fn harness_with(name: &str, search: SearchPath) -> Harness {
    let profile = tempfile::tempdir().expect("tempdir");
    let dirs = Dirs::new(profile.path().join("extensions"))
        .with_build_target(support::fixture_target_dir().join(format!("host-{name}")));
    let host = ExtensionHost::new(HostConfig {
        dirs,
        search_path: search,
        projects: Arc::new(Vec::new),
        on_change: Arc::new(|| {}),
        reserved_ids: vec!["claude-code".into()],
    })
    .expect("host");
    Harness { host, profile }
}

fn harness(name: &str) -> Harness {
    harness_with(name, SearchPath::from_env())
}

impl Harness {
    fn enable(&self, ids: &[&str], configs: HashMap<String, serde_json::Value>) {
        self.host.apply_settings(HostSettings {
            enabled: ids.iter().map(|s| s.to_string()).collect::<HashSet<_>>(),
            configs,
        });
    }

    /// Waits until `pred` holds for the extension.
    fn wait(&self, id: &str, what: &str, pred: impl Fn(&ApiExtension) -> bool) -> ApiExtension {
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            if let Some(ext) = self.host.extension(id)
                && pred(&ext)
            {
                return ext;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {what}: {:?}", self.host.extension(id));
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn wait_ready(&self, id: &str) -> ApiExtension {
        self.wait(id, "ready with a view", |e| e.state == ExtRunState::Ready && e.view.is_some() && !e.refreshing)
    }
}

fn status_label(ext: &ApiExtension) -> String {
    ext.status.as_ref().map(|s| s.label.clone()).unwrap_or_default()
}

#[test]
fn a_prebuilt_extension_installs_from_a_url_and_path_and_runs() {
    let Some(lib) = Library::new(true) else { return };
    let head = git(lib.path(), &["rev-parse", "HEAD"]);
    let h = harness("a_prebuilt_extension_installs_from_a_url_and_path_and_runs");

    let preview = h.host.preview_install(&lib.source()).expect("preview");
    assert_eq!(preview.id, "probe");
    assert!(preview.prebuilt);
    assert_eq!(preview.permissions.commands, vec!["echo".to_string()]);
    assert!(preview.permissions.start_agents, "starting agents is listed for approval");
    let ExtSource::Git { commit, .. } = &preview.source else { panic!() };
    assert_eq!(commit, &head);

    let record = h
        .host
        .install(&lib.source(), Some(commit), &preview.permissions)
        .expect("install");
    let ExtSource::Git { commit: installed, git_ref, .. } = &record.source else { panic!() };
    assert_eq!(installed, &head, "the resolved commit is recorded");
    assert_eq!(git_ref.as_deref(), Some("main"), "and so is the ref");

    h.enable(&["probe"], HashMap::new());
    let ext = h.wait_ready("probe");
    assert_eq!(status_label(&ext), "1 rows");
    assert!(matches!(ext.view.as_ref().map(|v| &v.nodes[0]), Some(ExtNode::Table { .. })));
}

#[test]
fn installing_needs_exactly_the_permissions_asked_for() {
    let Some(lib) = Library::new(true) else { return };
    let h = harness("installing_needs_exactly_the_permissions_asked_for");
    let preview = h.host.preview_install(&lib.source()).expect("preview");
    let mut less = preview.permissions.clone();
    less.start_agents = false;
    let err = h.host.install(&lib.source(), None, &less).expect_err("refused");
    assert!(err.contains("review it again"), "{err}");
    assert!(!h.host.is_installed("probe"));
}

#[test]
fn an_extension_without_a_prebuilt_component_is_built_from_source() {
    let Some(lib) = Library::new(false) else { return };
    let h = harness("an_extension_without_a_prebuilt_component_is_built_from_source");
    let preview = h.host.preview_install(&lib.source()).expect("preview");
    assert!(!preview.prebuilt);
    assert_eq!(preview.build_problem, None, "this machine can build it");
    h.host
        .install(&lib.source(), None, &preview.permissions)
        .expect("builds and installs");
    h.enable(&["probe"], HashMap::new());
    assert_eq!(status_label(&h.wait_ready("probe")), "1 rows");
}

#[cfg(unix)]
#[test]
fn building_without_rust_says_so_instead_of_failing_silently() {
    let Some(lib) = Library::new(false) else { return };
    // A PATH with git and nothing else.
    let bin = tempfile::tempdir().expect("tempdir");
    let real_git = SearchPath::from_env().resolve("git").expect("git");
    std::os::unix::fs::symlink(real_git, bin.path().join("git")).expect("symlink");
    let h = harness_with("building_without_rust_says_so_instead_of_failing_silently", SearchPath(Some(bin.path().as_os_str().to_owned())));

    let preview = h.host.preview_install(&lib.source()).expect("preview");
    let problem = preview.build_problem.expect("a build problem");
    assert!(problem.contains("needs Rust"), "{problem}");
    let err = h.host.install(&lib.source(), None, &preview.permissions).expect_err("fails");
    assert!(err.contains("needs Rust"), "{err}");
}

#[test]
fn a_moved_ref_shows_an_update_and_more_permissions_need_approval() {
    let Some(lib) = Library::new(true) else { return };
    let h = harness("a_moved_ref_shows_an_update_and_more_permissions_need_approval");
    let preview = h.host.preview_install(&lib.source()).expect("preview");
    let first = h.host.install(&lib.source(), None, &preview.permissions).expect("install");
    h.enable(&["probe"], HashMap::new());
    h.wait_ready("probe");
    assert!(h.host.check_updates().is_empty(), "nothing moved yet");

    // A new version with the same permissions updates without asking.
    lib.edit_manifest("version = \"0.1.0\"", "version = \"0.1.1\"");
    let second = lib.commit("0.1.1");
    assert_eq!(h.host.check_updates(), vec!["probe".to_string()]);
    let update = h.host.extension("probe").and_then(|e| e.update).expect("update");
    assert_eq!(update.commit, second);
    assert_eq!(update.version, "0.1.1");
    assert!(update.added_permissions.is_empty());
    let record = h.host.update("probe", Some(&update.commit), None).expect("update");
    assert_eq!(record.version, "0.1.1");
    assert_ne!(record.source, first.source);
    assert!(h.host.extension("probe").and_then(|e| e.update).is_none(), "the update is gone once installed");
    h.wait_ready("probe");

    // One that asks for another command must be approved first.
    lib.edit_manifest("commands = [\"echo\"]", "commands = [\"echo\", \"ls\"]");
    lib.edit_manifest("version = \"0.1.1\"", "version = \"0.2.0\"");
    lib.commit("0.2.0 runs ls");
    h.host.check_updates();
    let update = h.host.extension("probe").and_then(|e| e.update).expect("update");
    assert_eq!(update.added_permissions.commands, vec!["ls".to_string()]);
    let err = h.host.update("probe", Some(&update.commit), None).expect_err("needs approval");
    assert!(err.contains("approve") && err.contains("ls"), "{err}");
    assert_eq!(h.host.extension("probe").map(|e| e.version), Some("0.1.1".into()));

    let preview = h.host.preview_update("probe").expect("preview update");
    let record = h
        .host
        .update("probe", Some(&update.commit), Some(&preview.permissions))
        .expect("approved update");
    assert_eq!(record.version, "0.2.0");
    assert_eq!(record.approved.commands, vec!["echo".to_string(), "ls".to_string()]);
}

#[test]
fn installing_the_reviewed_commit_holds_even_if_the_ref_moves_meanwhile() {
    let Some(lib) = Library::new(true) else { return };
    let h = harness("installing_the_reviewed_commit_holds_even_if_the_ref_moves_meanwhile");
    let preview = h.host.preview_install(&lib.source()).expect("preview");
    let ExtSource::Git { commit: reviewed, .. } = preview.source.clone() else { panic!() };
    lib.edit_manifest("version = \"0.1.0\"", "version = \"0.9.0\"");
    lib.commit("moved");
    let record = h
        .host
        .install(&lib.source(), Some(&reviewed), &preview.permissions)
        .expect("install");
    assert_eq!(record.version, "0.1.0");
    let ExtSource::Git { commit, .. } = record.source else { panic!() };
    assert_eq!(commit, reviewed);
    assert_eq!(h.host.check_updates(), vec!["probe".to_string()], "and the newer commit shows as an update");
}

#[test]
fn removing_deletes_the_extension_and_its_data() {
    let Some(lib) = Library::new(true) else { return };
    let h = harness("removing_deletes_the_extension_and_its_data");
    let preview = h.host.preview_install(&lib.source()).expect("preview");
    h.host.install(&lib.source(), None, &preview.permissions).expect("install");
    h.enable(&["probe"], HashMap::new());
    h.wait_ready("probe");
    h.host
        .query("probe", "kv_set", r#"{"key":"k","value":"v"}"#)
        .expect("store something");
    let root = h.profile.path().join("extensions");
    assert!(root.join("data/probe/kv.json").is_file());

    h.host.remove("probe").expect("remove");
    assert!(!h.host.is_installed("probe"));
    assert!(!root.join("installed/probe").exists());
    assert!(!root.join("data/probe").exists());

    // And it stays gone for the next daemon.
    let again = ExtensionHost::new(HostConfig {
        dirs: Dirs::new(root),
        search_path: SearchPath::from_env(),
        projects: Arc::new(Vec::new),
        on_change: Arc::new(|| {}),
        reserved_ids: vec![],
    })
    .expect("host");
    assert!(again.installed_ids().is_empty());
}

#[test]
fn installed_extensions_come_back_after_a_restart() {
    let Some(lib) = Library::new(true) else { return };
    let h = harness("installed_extensions_come_back_after_a_restart");
    let preview = h.host.preview_install(&lib.source()).expect("preview");
    h.host.install(&lib.source(), None, &preview.permissions).expect("install");

    let again = ExtensionHost::new(HostConfig {
        dirs: Dirs::new(h.profile.path().join("extensions")),
        search_path: SearchPath::from_env(),
        projects: Arc::new(Vec::new),
        on_change: Arc::new(|| {}),
        reserved_ids: vec![],
    })
    .expect("host");
    assert_eq!(again.installed_ids(), vec!["probe".to_string()]);
    let ext = again.extension("probe").expect("listed");
    assert_eq!(ext.state, ExtRunState::Disabled);
    assert_eq!(ext.permissions, preview.permissions);
}

#[test]
fn a_local_folder_installs_and_reloads_after_a_rebuild_keeping_its_data() {
    let Some(lib) = Library::new(false) else { return };
    let h = harness("a_local_folder_installs_and_reloads_after_a_rebuild_keeping_its_data");
    let source = ExtSource::Local {
        path: lib.ext_dir().to_string_lossy().into_owned(),
    };
    let preview = h.host.preview_install(&source).expect("preview");
    h.host.install(&source, None, &preview.permissions).expect("install");
    h.enable(&["probe"], HashMap::new());
    h.wait_ready("probe");
    h.host
        .query("probe", "kv_set", r#"{"key":"k","value":"kept"}"#)
        .expect("store");

    // Edit the source: the status label changes.
    let lib_rs = lib.ext_dir().join("src/lib.rs");
    let code = std::fs::read_to_string(&lib_rs).expect("read");
    std::fs::write(&lib_rs, code.replace("\"{} rows\"", "\"{} rows (reloaded)\"")).expect("write");

    h.host.reload("probe", None).expect("reload");
    let ext = h.wait(
        "probe",
        "the reloaded status",
        |e| e.state == ExtRunState::Ready && status_label(e) == "1 rows (reloaded)",
    );
    assert!(matches!(ext.source, ExtSource::Local { .. }));
    assert_eq!(
        h.host.query("probe", "kv_get", r#"{"key":"k"}"#).expect("get"),
        "\"kept\""
    );
}

#[test]
fn a_built_in_extension_id_cannot_be_taken() {
    let Some(lib) = Library::new(true) else { return };
    lib.edit_manifest("id = \"probe\"", "id = \"claude-code\"");
    lib.commit("impostor");
    let h = harness("a_built_in_extension_id_cannot_be_taken");
    let err = h.host.preview_install(&lib.source()).expect_err("refused");
    assert!(err.contains("built into okena"), "{err}");
}

#[test]
fn a_missing_tool_blocks_the_extension_until_a_recheck_passes() {
    let Some(lib) = Library::new(true) else { return };
    // Require a tool that a test-controlled folder provides.
    let tools = tempfile::tempdir().expect("tempdir");
    lib.edit_manifest(
        "[[config]]",
        "[[requires]]\nname = \"frob\"\ncheck = [\"frob\", \"--version\"]\ninstall_hint = \"brew install frob\"\n\n[[config]]",
    );
    lib.commit("needs frob");
    let path = std::env::join_paths(
        std::iter::once(tools.path().to_path_buf()).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())),
    )
    .expect("join");
    let h = harness_with("a_missing_tool_blocks_the_extension_until_a_recheck_passes", SearchPath(Some(path)));
    let preview = h.host.preview_install(&lib.source()).expect("preview");
    assert_eq!(preview.requires[0].name, "frob");
    h.host.install(&lib.source(), None, &preview.permissions).expect("install");
    h.enable(&["probe"], HashMap::new());

    let ext = h.wait("probe", "missing tools", |e| e.state == ExtRunState::MissingTools);
    assert_eq!(ext.tools[0].install_hint, "brew install frob");
    assert!(ext.view.is_none(), "nothing ran");
    let err = h.host.query("probe", "count", "{}").expect_err("not ready");
    assert!(err.contains("missing required tools"), "{err}");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let frob = tools.path().join("frob");
        std::fs::write(&frob, "#!/bin/sh\necho 'frob 1.0.0'\n").expect("write");
        std::fs::set_permissions(&frob, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        h.host.recheck("probe").expect("recheck");
        let ext = h.wait_ready("probe");
        assert!(ext.tools[0].ok);
    }
}

#[test]
fn a_required_config_field_holds_it_back_and_a_change_applies_without_restarting() {
    let Some(lib) = Library::new(true) else { return };
    lib.edit_manifest("type = \"path\"", "type = \"path\"\nrequired = true");
    lib.commit("dir is required");
    let h = harness("a_required_config_field_holds_it_back_and_a_change_applies_without_restarting");
    let preview = h.host.preview_install(&lib.source()).expect("preview");
    h.host.install(&lib.source(), None, &preview.permissions).expect("install");
    h.enable(&["probe"], HashMap::new());
    let ext = h.wait("probe", "needs config", |e| matches!(e.state, ExtRunState::NeedsConfig { .. }));
    assert_eq!(ext.state, ExtRunState::NeedsConfig { missing: vec!["dir".into()] });

    let data = tempfile::tempdir().expect("tempdir");
    let dir = data.path().to_string_lossy().to_string();
    h.enable(&["probe"], HashMap::from([("probe".to_string(), serde_json::json!({ "dir": dir }))]));
    h.wait_ready("probe");
    let config = h.host.query("probe", "config", "{}").expect("config");
    assert!(config.contains(&dir), "{config}");
}

#[test]
fn disabling_stops_the_extension_and_enabling_starts_it_again() {
    let Some(lib) = Library::new(true) else { return };
    let h = harness("disabling_stops_the_extension_and_enabling_starts_it_again");
    let preview = h.host.preview_install(&lib.source()).expect("preview");
    h.host.install(&lib.source(), None, &preview.permissions).expect("install");
    h.enable(&["probe"], HashMap::new());
    h.wait_ready("probe");
    h.enable(&[], HashMap::new());
    let ext = h.host.extension("probe").expect("still installed");
    assert_eq!(ext.state, ExtRunState::Disabled);
    assert!(!ext.enabled);
    assert!(h.host.refresh("probe").is_err());
    h.enable(&["probe"], HashMap::new());
    h.wait_ready("probe");
}

#[test]
fn approved_permissions_not_the_manifest_bound_the_running_extension() {
    // Sanity: the permissions shown and stored are the approved ones.
    let Some(lib) = Library::new(true) else { return };
    let h = harness("approved_permissions_not_the_manifest_bound_the_running_extension");
    let preview = h.host.preview_install(&lib.source()).expect("preview");
    h.host.install(&lib.source(), None, &preview.permissions).expect("install");
    h.enable(&["probe"], HashMap::new());
    h.wait_ready("probe");
    let err = h
        .host
        .query("probe", "run", r#"{"program":"ls"}"#)
        .expect_err("refused");
    assert!(err.contains("refused"), "{err}");
    let ext = h.wait("probe", "the refusal on show", |e| !e.refusals.is_empty());
    assert!(ext.refusals[0].message.contains("`ls`"));
    assert_eq!(ext.permissions, ExtPermissions { ..preview.permissions });
}

#[test]
fn an_extension_refreshes_on_its_declared_interval() {
    let Some(lib) = Library::new(true) else { return };
    lib.edit_manifest("refresh_interval_secs = 0", "refresh_interval_secs = 5");
    lib.commit("refresh every 5s");
    let h = harness("an_extension_refreshes_on_its_declared_interval");
    let preview = h.host.preview_install(&lib.source()).expect("preview");
    h.host.install(&lib.source(), None, &preview.permissions).expect("install");
    h.enable(&["probe"], HashMap::new());
    let first = h.wait_ready("probe").refreshed_at_ms.expect("refreshed once");
    assert_eq!(h.host.extension("probe").map(|e| e.refresh_interval_secs), Some(5));
    // No one asks; the interval alone refreshes it again.
    let again = h.wait("probe", "a second refresh", |e| e.refreshed_at_ms.is_some_and(|t| t > first));
    let gap = again.refreshed_at_ms.unwrap_or_default() - first;
    assert!((4_000..10_000).contains(&gap), "refreshed {gap}ms after the first");
}

#[test]
fn an_agents_destructive_call_waits_for_the_users_answer() {
    let Some(lib) = Library::new(true) else { return };
    let h = harness("an_agents_destructive_call_waits_for_the_users_answer");
    let preview = h.host.preview_install(&lib.source()).expect("preview");
    h.host.install(&lib.source(), None, &preview.permissions).expect("install");
    h.enable(&["probe"], HashMap::new());
    h.wait_ready("probe");
    let wipe = h.host.action_def("probe", "wipe").expect("wipe");
    let host = Arc::new(h);

    for approve in [true, false] {
        let waiting = {
            let host = host.clone();
            let wipe = wipe.clone();
            std::thread::spawn(move || host.host.await_confirmation("probe", &wipe, &["a".into()], None))
        };
        let pending = host.wait("probe", "a pending confirmation", |e| !e.pending_confirmations.is_empty());
        let request = &pending.pending_confirmations[0];
        assert_eq!(request.action_label, "Wipe");
        assert_eq!(request.items, vec!["a".to_string()]);
        host.host.confirm("probe", &request.id, approve).expect("answer");
        assert_eq!(waiting.join().expect("join"), approve);
        let after = host.host.extension("probe").expect("ext");
        assert!(after.pending_confirmations.is_empty(), "answered requests leave the snapshot");
        assert!(host.host.confirm("probe", &request.id, true).is_err(), "and cannot be answered twice");
    }
}
