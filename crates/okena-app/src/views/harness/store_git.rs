//! A store checkout's git, as the Specs and Knowledge views show it: where the
//! branch stands, Fetch, Pull and Push with the reason any of them is
//! unavailable, and the uncommitted files with a box to commit them.
//!
//! Both views render this one panel over the same `StoreGitStatus` (ADR-0004);
//! they differ only in the actions posted and the view refreshed after. Every
//! operation goes through the daemon, which resolves the root key itself and
//! refuses to commit any path its own status did not list.

use super::HarnessPane;
use crate::theme::theme;
use crate::ui::tokens::{ui_text, ui_text_md, ui_text_ms};
use crate::views::components::{SimpleInput, SimpleInputState};
use gpui::prelude::*;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_core::api::ActionRequest;
use okena_core::store_git::{
    MAX_LISTED_CHANGES, StoreChange, StoreChangeKind, StoreGitStatus, default_commit_message,
};

/// Which view's open store an operation acts on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StoreSection {
    Specs,
    Knowledge,
}

/// Element ids for one view's controls.
struct Ids {
    fetch: &'static str,
    pull: &'static str,
    push: &'static str,
    commit: &'static str,
}

impl StoreSection {
    fn ids(self) -> Ids {
        match self {
            StoreSection::Specs => Ids {
                fetch: "spec-store-fetch",
                pull: "spec-store-pull",
                push: "spec-store-push",
                commit: "spec-store-commit",
            },
            StoreSection::Knowledge => Ids {
                fetch: "knowledge-fetch",
                pull: "knowledge-pull",
                push: "knowledge-push",
                commit: "knowledge-commit",
            },
        }
    }
}

/// A git operation on the open store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum StoreOp {
    Fetch,
    Pull,
    Commit { paths: Vec<String>, message: String },
    Push,
}

impl StoreOp {
    pub(crate) fn into_request(self, section: StoreSection, root: String) -> ActionRequest {
        match (section, self) {
            (StoreSection::Specs, StoreOp::Fetch) => ActionRequest::SpecStoreFetch { root },
            (StoreSection::Specs, StoreOp::Pull) => ActionRequest::SpecStorePull { root },
            (StoreSection::Specs, StoreOp::Commit { paths, message }) => {
                ActionRequest::SpecStoreCommit {
                    root,
                    paths,
                    message,
                }
            }
            (StoreSection::Specs, StoreOp::Push) => ActionRequest::SpecStorePush { root },
            (StoreSection::Knowledge, StoreOp::Fetch) => {
                ActionRequest::KnowledgeStoreFetch { root }
            }
            (StoreSection::Knowledge, StoreOp::Pull) => ActionRequest::KnowledgeStorePull { root },
            (StoreSection::Knowledge, StoreOp::Commit { paths, message }) => {
                ActionRequest::KnowledgeStoreCommit {
                    root,
                    paths,
                    message,
                }
            }
            (StoreSection::Knowledge, StoreOp::Push) => ActionRequest::KnowledgeStorePush { root },
        }
    }

    fn busy_label(&self) -> &'static str {
        match self {
            StoreOp::Fetch => "Fetching…",
            StoreOp::Pull => "Pulling…",
            StoreOp::Commit { .. } => "Committing…",
            StoreOp::Push => "Pushing…",
        }
    }
}

/// One view's store git state.
pub(crate) struct StoreGitPanel {
    /// The running operation's busy label. One at a time: each takes git's
    /// index or ref locks, and a second would only fail on them.
    pub(crate) running: Option<&'static str>,
    /// Root key the last outcome or error belongs to, so switching roots does
    /// not show another store's result.
    pub(crate) root: Option<String>,
    /// What the last operation did, e.g. "Pulled 3 commits".
    pub(crate) outcome: Option<String>,
    pub(crate) error: Option<String>,
    pub(crate) commit_message: Entity<SimpleInputState>,
}

impl StoreGitPanel {
    pub(crate) fn new(cx: &mut Context<HarnessPane>) -> Self {
        let commit_message = cx.new(|cx| SimpleInputState::new(cx).placeholder("Commit message"));
        Self {
            running: None,
            root: None,
            outcome: None,
            error: None,
            commit_message,
        }
    }
}

// ─── Pure helpers ───────────────────────────────────────────────────────────

/// `↑2 ↓3 •` — commits to push, commits to pull, uncommitted changes. `None`
/// when there is nothing to say.
pub(crate) fn sync_badge(git: &StoreGitStatus) -> Option<String> {
    let mut parts = Vec::new();
    if git.ahead > 0 {
        parts.push(format!("↑{}", git.ahead));
    }
    if git.behind > 0 {
        parts.push(format!("↓{}", git.behind));
    }
    if git.dirty {
        parts.push("•".to_string());
    }
    (!parts.is_empty()).then(|| parts.join(" "))
}

/// "fetched 2h ago", from Unix seconds.
pub(crate) fn fetched_ago(now: u64, fetched_at: Option<u64>) -> String {
    let Some(at) = fetched_at else {
        return "never fetched".to_string();
    };
    let secs = now.saturating_sub(at);
    match secs {
        0..60 => "fetched just now".to_string(),
        60..3_600 => format!("fetched {}m ago", secs / 60),
        3_600..86_400 => format!("fetched {}h ago", secs / 3_600),
        _ => format!("fetched {}d ago", secs / 86_400),
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// The changes a commit takes: every listed one but conflicts, which only a
/// terminal can resolve (the daemon refuses them too).
pub(crate) fn committable(git: &StoreGitStatus) -> Vec<StoreChange> {
    git.changes
        .iter()
        .filter(|c| c.kind != StoreChangeKind::Conflicted)
        .cloned()
        .collect()
}

/// What a finished operation says, from the sync state before and after it.
pub(crate) fn outcome(
    op: &StoreOp,
    before: Option<&StoreGitStatus>,
    after: &StoreGitStatus,
) -> String {
    let commits = |n: u32| plural(n as usize, "commit", "commits");
    match op {
        StoreOp::Fetch if after.behind > 0 => format!("{} to pull", commits(after.behind)),
        StoreOp::Fetch => "Up to date".to_string(),
        StoreOp::Pull if after.behind > 0 => format!("{} still to pull", commits(after.behind)),
        StoreOp::Pull => match before.map_or(0, |b| b.behind) {
            0 => "Up to date".to_string(),
            n => format!("Pulled {}", commits(n)),
        },
        StoreOp::Commit { paths, .. } => {
            let files = plural(paths.len(), "file", "files");
            if after.upstream.is_some() && after.ahead > 0 {
                format!("Committed {files}; {} to push", commits(after.ahead))
            } else {
                format!("Committed {files}")
            }
        }
        StoreOp::Push => match before.map_or(0, |b| b.ahead) {
            0 => "Nothing to push".to_string(),
            n => format!("Pushed {}", commits(n)),
        },
    }
}

fn branch_line(git: &StoreGitStatus) -> String {
    match (&git.branch, &git.upstream) {
        (Some(b), Some(u)) => format!("{b} → {u}"),
        (Some(b), None) => format!("{b} (no upstream)"),
        (None, _) => "detached HEAD".to_string(),
    }
}

fn change_mark(kind: StoreChangeKind) -> &'static str {
    match kind {
        StoreChangeKind::Added => "A",
        StoreChangeKind::Modified => "M",
        StoreChangeKind::Deleted => "D",
        StoreChangeKind::Untracked => "?",
        StoreChangeKind::Conflicted => "!",
    }
}

// ─── Actions ────────────────────────────────────────────────────────────────

impl HarnessPane {
    fn store_panel(&self, section: StoreSection) -> &StoreGitPanel {
        match section {
            StoreSection::Specs => &self.specs.git,
            StoreSection::Knowledge => &self.knowledge.git,
        }
    }

    fn store_panel_mut(&mut self, section: StoreSection) -> &mut StoreGitPanel {
        match section {
            StoreSection::Specs => &mut self.specs.git,
            StoreSection::Knowledge => &mut self.knowledge.git,
        }
    }

    /// The open root's key, and the sync state the view last listed for it.
    fn open_store(&self, section: StoreSection) -> Option<(String, Option<StoreGitStatus>)> {
        match section {
            StoreSection::Specs => {
                let key = self.specs.root_key.clone()?;
                let git = self
                    .specs
                    .stores
                    .as_ref()
                    .and_then(|s| s.root(&key))
                    .and_then(|r| r.git.clone());
                Some((key, git))
            }
            StoreSection::Knowledge => {
                let key = self.knowledge.root_key.clone()?;
                let git = self
                    .knowledge
                    .stores
                    .as_ref()
                    .and_then(|s| s.root(&key))
                    .and_then(|r| r.git.clone());
                Some((key, git))
            }
        }
    }

    /// Run `op` on the open store, then re-list so the badge, the changes and
    /// the entries catch up.
    fn run_store_op(&mut self, section: StoreSection, op: StoreOp, cx: &mut Context<Self>) {
        let Some((root, before)) = self.open_store(section) else {
            return;
        };
        let panel = self.store_panel_mut(section);
        if panel.running.is_some() {
            return;
        }
        panel.running = Some(op.busy_label());
        panel.root = Some(root.clone());
        panel.outcome = None;
        panel.error = None;
        cx.notify();

        let client = self.client.clone();
        let request = op.clone().into_request(section, root);
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                client
                    .post_action(request)
                    .and_then(|v| v.ok_or_else(|| "Missing sync state".to_string()))
                    .and_then(|v| {
                        serde_json::from_value::<StoreGitStatus>(v)
                            .map_err(|e| format!("Unexpected sync state: {e}"))
                    })
            })
            .await;

            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    let panel = this.store_panel_mut(section);
                    panel.running = None;
                    let committed = match &result {
                        Ok(after) => {
                            panel.outcome = Some(outcome(&op, before.as_ref(), after));
                            matches!(op, StoreOp::Commit { .. })
                        }
                        Err(e) => {
                            panel.error = Some(e.clone());
                            false
                        }
                    };
                    if committed {
                        let input = panel.commit_message.clone();
                        input.update(cx, |i, cx| i.set_value("", cx));
                    }
                    // Re-list after a failure too: a pull that fetched first,
                    // or a push refused after its commit, still changed what
                    // there is to show.
                    match section {
                        StoreSection::Specs => this.refresh_specs(cx),
                        StoreSection::Knowledge => this.refresh_knowledge(cx),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Commit every committable change listed for the open store, with the
    /// typed message or else the default one the hint offers.
    fn commit_store(&mut self, section: StoreSection, cx: &mut Context<Self>) {
        let Some((_, Some(git))) = self.open_store(section) else {
            return;
        };
        let changes = committable(&git);
        if changes.is_empty() {
            return;
        }
        let typed = self
            .store_panel(section)
            .commit_message
            .read(cx)
            .value()
            .trim()
            .to_string();
        let message = if typed.is_empty() {
            default_commit_message(&changes)
        } else {
            typed
        };
        let paths = changes.into_iter().map(|c| c.path).collect();
        self.run_store_op(section, StoreOp::Commit { paths, message }, cx);
    }
}

// ─── Rendering ──────────────────────────────────────────────────────────────

impl HarnessPane {
    fn store_button(
        &self,
        id: &'static str,
        label: &str,
        enabled: bool,
        on_click: impl Fn(&mut Self, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if enabled {
            return self.small_button(
                id,
                label,
                cx.listener(move |this, _, _window, cx| on_click(this, cx)),
                cx,
            );
        }
        let t = theme(cx);
        div()
            .id(id)
            .flex_shrink_0()
            .px(px(10.0))
            .py(px(3.0))
            .rounded(px(4.0))
            .bg(rgb(t.bg_secondary))
            .text_size(ui_text_md(cx))
            .text_color(rgb(t.text_muted))
            .child(label.to_string())
            .into_any_element()
    }

    fn store_note(
        &self,
        text: impl Into<SharedString>,
        color: u32,
        cx: &Context<Self>,
    ) -> AnyElement {
        div()
            .text_size(ui_text_ms(cx))
            .text_color(rgb(color))
            .child(text.into())
            .into_any_element()
    }

    /// The open store's git: branch and sync facts, Fetch / Pull / Push with
    /// why each is unavailable, and the uncommitted files with a commit box.
    pub(super) fn render_store_git(
        &self,
        section: StoreSection,
        git: &StoreGitStatus,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let ids = section.ids();
        let panel = self.store_panel(section);
        let busy = panel.running.is_some();
        let open_root = self.open_store(section).map(|(key, _)| key);
        let own_result = panel.root.is_some() && panel.root == open_root;

        let state = match sync_badge(git) {
            Some(badge) => badge,
            None if git.upstream.is_some() => "in sync".to_string(),
            None => "clean".to_string(),
        };
        let mut col = v_flex()
            .gap(px(8.0))
            .child(self.fact_row("Branch", branch_line(git), cx))
            .child(self.fact_row(
                "Sync",
                format!("{state} · {}", fetched_ago(now_secs(), git.fetched_at)),
                cx,
            ));

        let mut buttons = h_flex()
            .gap(px(6.0))
            .pt(px(4.0))
            .items_center()
            .child(self.store_button(
                ids.fetch,
                "Fetch",
                !busy && git.upstream.is_some(),
                move |this, cx| this.run_store_op(section, StoreOp::Fetch, cx),
                cx,
            ))
            .child(self.store_button(
                ids.pull,
                "Pull",
                !busy && git.can_fast_forward(),
                move |this, cx| this.run_store_op(section, StoreOp::Pull, cx),
                cx,
            ))
            .child(self.store_button(
                ids.push,
                "Push",
                !busy && git.can_push(),
                move |this, cx| this.run_store_op(section, StoreOp::Push, cx),
                cx,
            ));
        if let Some(label) = panel.running {
            buttons = buttons.child(self.store_note(label, t.text_muted, cx));
        }
        col = col.child(buttons);

        // A reason only where there is something the button would otherwise
        // do: "nothing to pull" beside a clean, synced store is noise.
        if (git.behind > 0 || git.dirty)
            && let Some(why) = git.pull_blocker()
        {
            col = col.child(self.store_note(format!("Pull: {why}"), t.text_muted, cx));
        }
        if git.ahead > 0
            && let Some(why) = git.push_blocker()
        {
            col = col.child(self.store_note(format!("Push: {why}"), t.text_muted, cx));
        }
        if own_result {
            if let Some(outcome) = panel.outcome.clone() {
                col = col.child(self.store_note(outcome, t.text_secondary, cx));
            }
            if let Some(error) = panel.error.clone() {
                col = col.child(self.store_note(error, t.error, cx));
            }
        }
        if git.dirty {
            col = col.child(self.render_store_changes(section, git, cx));
        }
        col.into_any_element()
    }

    fn render_store_changes(
        &self,
        section: StoreSection,
        git: &StoreGitStatus,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let panel = self.store_panel(section);
        let mut col = v_flex().gap(px(3.0)).pt(px(8.0)).child(
            div()
                .pb(px(2.0))
                .text_size(ui_text_ms(cx))
                .text_color(rgb(t.text_muted))
                .child(format!("UNCOMMITTED CHANGES ({})", git.changes.len())),
        );
        if git.changes.is_empty() {
            return col
                .child(self.store_note(
                    "git could not list the changes; look at the checkout in a terminal.",
                    t.warning,
                    cx,
                ))
                .into_any_element();
        }

        for change in &git.changes {
            let color = match change.kind {
                StoreChangeKind::Added | StoreChangeKind::Untracked => t.success,
                StoreChangeKind::Modified => t.warning,
                StoreChangeKind::Deleted | StoreChangeKind::Conflicted => t.error,
            };
            col = col.child(
                h_flex()
                    .gap(px(8.0))
                    .min_w_0()
                    .items_center()
                    .child(
                        div()
                            .w(px(12.0))
                            .flex_shrink_0()
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(color))
                            .child(change_mark(change.kind)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(ui_text_md(cx))
                            .text_color(rgb(t.text_primary))
                            .child(change.path.clone()),
                    )
                    .when(change.staged, |row| {
                        row.child(self.store_note("staged", t.text_muted, cx))
                    }),
            );
        }
        if git.changes_truncated {
            col = col.child(self.store_note(
                format!(
                    "Only the first {MAX_LISTED_CHANGES} are listed; commit the rest in a terminal."
                ),
                t.text_muted,
                cx,
            ));
        }

        let changes = committable(git);
        let conflicted = git.changes.len() - changes.len();
        if conflicted > 0 {
            col = col.child(self.store_note(
                format!(
                    "{} left out; resolve conflicts in a terminal.",
                    plural(conflicted, "conflicted file is", "conflicted files are")
                ),
                t.warning,
                cx,
            ));
        }
        if changes.is_empty() {
            return col.into_any_element();
        }

        let label = format!("Commit {}", plural(changes.len(), "file", "files"));
        col.child(
            v_flex()
                .gap(px(5.0))
                .pt(px(6.0))
                .child(
                    okena_ui::input::input_container(&t, None)
                        .w_full()
                        .px(px(8.0))
                        .py(px(4.0))
                        .child(
                            SimpleInput::new(&panel.commit_message).text_size(ui_text(13.0, cx)),
                        ),
                )
                .child(self.store_note(
                    format!(
                        "Leave blank to commit as “{}”. Nothing is pushed until you push.",
                        default_commit_message(&changes)
                    ),
                    t.text_muted,
                    cx,
                ))
                .child(h_flex().child(self.store_button(
                    section.ids().commit,
                    &label,
                    panel.running.is_none(),
                    move |this, cx| this.commit_store(section, cx),
                    cx,
                ))),
        )
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    // Not `use super::*`: the gpui glob would shadow `#[test]` with
    // `gpui::test`, which expands into itself forever.
    use super::{StoreOp, StoreSection, committable, fetched_ago, outcome, sync_badge};
    use okena_core::api::ActionRequest;
    use okena_core::store_git::{StoreChange, StoreChangeKind, StoreGitStatus};

    fn git(ahead: u32, behind: u32, dirty: bool) -> StoreGitStatus {
        StoreGitStatus {
            branch: Some("main".into()),
            upstream: Some("origin/main".into()),
            ahead,
            behind,
            dirty,
            ..Default::default()
        }
    }

    #[test]
    fn the_sync_badge_says_only_what_needs_attention() {
        assert_eq!(sync_badge(&git(0, 0, false)), None);
        assert_eq!(sync_badge(&git(0, 3, false)).as_deref(), Some("↓3"));
        assert_eq!(sync_badge(&git(2, 3, true)).as_deref(), Some("↑2 ↓3 •"));
        assert_eq!(sync_badge(&git(0, 0, true)).as_deref(), Some("•"));
    }

    #[test]
    fn fetch_age_reads_in_the_largest_whole_unit() {
        let now = 1_000_000;
        assert_eq!(fetched_ago(now, None), "never fetched");
        assert_eq!(fetched_ago(now, Some(now - 5)), "fetched just now");
        assert_eq!(fetched_ago(now, Some(now - 150)), "fetched 2m ago");
        assert_eq!(fetched_ago(now, Some(now - 7_300)), "fetched 2h ago");
        assert_eq!(fetched_ago(now, Some(now - 200_000)), "fetched 2d ago");
        // A clock that moved backwards is not "in the future".
        assert_eq!(fetched_ago(now, Some(now + 60)), "fetched just now");
    }

    #[test]
    fn a_commit_leaves_conflicts_out() {
        let mut status = git(0, 0, true);
        status.changes = [
            ("docs/a.md", StoreChangeKind::Modified),
            ("docs/b.md", StoreChangeKind::Conflicted),
            ("docs/c.md", StoreChangeKind::Untracked),
        ]
        .into_iter()
        .map(|(path, kind)| StoreChange {
            path: path.into(),
            kind,
            staged: false,
            unstaged: true,
        })
        .collect();
        let paths: Vec<_> = committable(&status).into_iter().map(|c| c.path).collect();
        assert_eq!(paths, ["docs/a.md", "docs/c.md"]);
    }

    #[test]
    fn outcomes_say_what_changed() {
        let commit = StoreOp::Commit {
            paths: vec!["docs/a.md".into()],
            message: "m".into(),
        };
        assert_eq!(
            outcome(&commit, Some(&git(0, 0, true)), &git(1, 0, false)),
            "Committed 1 file; 1 commit to push"
        );
        let local = StoreGitStatus {
            upstream: None,
            ..git(0, 0, false)
        };
        assert_eq!(outcome(&commit, None, &local), "Committed 1 file");
        assert_eq!(
            outcome(&StoreOp::Push, Some(&git(2, 0, false)), &git(0, 0, false)),
            "Pushed 2 commits"
        );
        assert_eq!(
            outcome(&StoreOp::Pull, Some(&git(0, 3, false)), &git(0, 0, false)),
            "Pulled 3 commits"
        );
        assert_eq!(
            outcome(&StoreOp::Fetch, None, &git(0, 1, false)),
            "1 commit to pull"
        );
        assert_eq!(
            outcome(&StoreOp::Fetch, None, &git(0, 0, false)),
            "Up to date"
        );
    }

    #[test]
    fn each_section_posts_its_own_actions() {
        let root = "store:x".to_string();
        assert!(matches!(
            StoreOp::Push.into_request(StoreSection::Specs, root.clone()),
            ActionRequest::SpecStorePush { .. }
        ));
        assert!(matches!(
            StoreOp::Commit {
                paths: vec!["a".into()],
                message: "m".into()
            }
            .into_request(StoreSection::Knowledge, root),
            ActionRequest::KnowledgeStoreCommit { ref paths, .. } if paths == &["a".to_string()]
        ));
    }
}
