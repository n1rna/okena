//! One row of a session's PRODUCED list.
//!
//! Shared by the session's own panel and the task detail, which list the same
//! rows and had drifted into two copies of the same markup.
//!
//! Each kind of asset gets the component it calls for, in one shared frame: an
//! icon for the kind coloured by where it stands, and its actions as icon
//! buttons. A PR row says whether it can merge: its state, conflicts, the CI
//! rollup, the review decision and unresolved threads, each a compact chip,
//! with the PR's page a click away. The CI chip opens the same checks list the
//! project header's CI popover shows.

use gpui::prelude::*;
use gpui::*;
use gpui_component::popover::Popover;
use gpui_component::{Selectable, h_flex, v_flex};
use okena_core::api::{MergeState, PrInfo, PrReadiness, PrState, ReviewDecision};
use okena_core::harness::AgentAssetKind;
use okena_core::session_assets::{DetectedState, SessionAsset};
use okena_views_git::git_header::{render_ci_checks_header, render_ci_checks_list};

use super::worktree_card::{PushState, chip, ci_chip_style, pr_chip_style};
use crate::theme::{theme, with_alpha};
use crate::ui::tokens::ui_text_ms;

/// Render one produced asset on `bg`, as the component its kind calls for.
///
/// Every kind shares one frame — an icon for the kind, coloured by where it
/// stands, a title and caption, and its actions as icon buttons — and differs
/// in the facts and actions it carries: a PR its state, checks and what is in
/// the way of merging it; a branch its push state; a task where it stands.
pub fn render_asset_row(asset: &SessionAsset, bg: u32, cx: &App) -> AnyElement {
    let t = theme(cx);
    let pr_state = pr_state(asset);

    let mut chips: Vec<AnyElement> = Vec::new();
    if let Some((number, state)) = &pr_state {
        let (color, label) = pr_chip_style(*number, state, &t);
        chips.push(chip(label, color, cx));
    }
    if let Some(DetectedState::LocalOnly) = &asset.state {
        chips.push(chip("local only".into(), t.warning, cx));
    }
    if let Some(DetectedState::Pushed {
        unpushed,
        ahead,
        behind,
    }) = &asset.state
    {
        let push = PushState::from_unpushed(Some(*unpushed));
        chips.push(chip(
            push.label(),
            if push.is_pending() {
                t.warning
            } else {
                t.success
            },
            cx,
        ));
        if let Some(counts) = ahead_behind(*ahead, *behind) {
            chips.push(chip(counts, t.text_muted, cx));
        }
    }
    if let Some(pr) = asset.pr.as_ref() {
        chips.extend(readiness_chips(pr, cx));
    }
    let key = row_key(asset);
    if let Some(summary) = asset.ci.clone() {
        chips.push(ci_checks_chip(
            SharedString::from(format!("asset-ci-{key}")),
            summary,
            asset.pr.clone(),
            cx,
        ));
    }
    if let Some(changes) = asset.uncommitted {
        chips.push(chip(
            format!("+{} −{} uncommitted", changes.added, changes.removed),
            t.text_muted,
            cx,
        ));
    }
    let task_state = crate::views::known_tasks::task_state(asset.task.as_ref(), cx);
    if asset.kind == AgentAssetKind::Task
        && let Some(state) = task_state.clone()
    {
        chips.push(chip(state, t.text_secondary, cx));
    }

    let icon_color = match (&asset.kind, &pr_state, &asset.state) {
        (_, Some((_, state)), _) => pr_chip_style(0, state, &t).0,
        (_, None, Some(DetectedState::LocalOnly)) => t.warning,
        (_, None, Some(DetectedState::Pushed { unpushed, .. })) if *unpushed > 0 => t.warning,
        (_, None, Some(DetectedState::Pushed { .. })) => t.success,
        _ => t.text_muted,
    };

    let buttons: Vec<AnyElement> = actions(asset)
        .into_iter()
        .enumerate()
        .map(|(i, action)| {
            let (icon, tip) = match &action {
                Action::Open { label, .. } => ("icons/external-link.svg", *label),
                Action::Copy { label, .. } => ("icons/copy.svg", *label),
            };
            okena_ui::icon_button::icon_button_sized(
                SharedString::from(format!("asset-action-{key}-{i}")),
                icon,
                22.0,
                13.0,
                &t,
            )
            .tooltip(move |window, cx| gpui_component::tooltip::Tooltip::new(tip).build(window, cx))
            .on_click(move |_, _window, cx| match &action {
                Action::Open { url, .. } => cx.open_url(url),
                Action::Copy { text, done, .. } => {
                    cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                    crate::workspace::toast::ToastManager::success(*done, cx);
                }
            })
            .into_any_element()
        })
        .collect();

    v_flex()
        .w_full()
        .min_w_0()
        .gap(px(4.0))
        .px(px(8.0))
        .py(px(6.0))
        .rounded(px(6.0))
        .bg(rgb(bg))
        .border_1()
        .border_color(rgb(t.border))
        .child(
            h_flex()
                .w_full()
                .min_w_0()
                .items_start()
                .gap(px(7.0))
                .child(
                    svg()
                        .path(kind_icon(&asset.kind))
                        .flex_shrink_0()
                        .mt(px(2.0))
                        .size(px(13.0))
                        .text_color(rgb(icon_color)),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap(px(1.0))
                        .child(
                            div()
                                .w_full()
                                .min_w_0()
                                .truncate()
                                .text_size(ui_text_ms(cx))
                                .text_color(rgb(t.text_primary))
                                .child(asset.title.clone()),
                        )
                        .child(
                            div()
                                .w_full()
                                .min_w_0()
                                .truncate()
                                .text_size(ui_text_ms(cx))
                                .text_color(rgb(t.text_muted))
                                .child(subtitle(asset, task_state.as_deref())),
                        ),
                )
                .child(
                    h_flex()
                        .flex_shrink_0()
                        .items_center()
                        .gap(px(2.0))
                        .children(buttons),
                ),
        )
        .when(!chips.is_empty(), |row| {
            row.child(
                h_flex()
                    .pl(px(20.0))
                    .gap(px(4.0))
                    .flex_wrap()
                    .children(chips),
            )
        })
        .into_any_element()
}

/// What stands between `pr` and merging it, one chip each: conflicts, the
/// review decision, unresolved threads. Shared by every PR row, so a PR reads
/// the same wherever it is listed.
pub fn readiness_chips(pr: &PrInfo, cx: &App) -> Vec<AnyElement> {
    let t = theme(cx);
    pr_indicators(pr)
        .into_iter()
        .map(|(label, tone)| {
            let color = match tone {
                Tone::Bad => t.error,
                Tone::Warn => t.warning,
                Tone::Good => t.success,
                Tone::Muted => t.text_muted,
            };
            chip(label, color, cx)
        })
        .collect()
}

/// The CI rollup as a chip that opens the checks list — the same list the
/// project header's CI popover shows. `id` must be unique among its siblings.
pub fn ci_checks_chip(
    id: SharedString,
    summary: okena_core::api::CiCheckSummary,
    pr: Option<PrInfo>,
    cx: &App,
) -> AnyElement {
    let t = theme(cx);
    let (color, label) = ci_chip_style(
        &summary.status,
        summary.passed,
        summary.failed,
        summary.pending,
        &t,
    );
    Popover::new(id)
        .trigger(CiTrigger {
            label,
            color,
            selected: false,
        })
        .content(move |_, _window, cx| {
            let t = theme(cx);
            v_flex()
                .w(px(360.0))
                .max_h(px(420.0))
                .child(render_ci_checks_header(&summary, pr.as_ref(), &t, cx))
                .child(render_ci_checks_list(&summary, &t, cx))
        })
        .into_any_element()
}

/// A stable id fragment for one row, for element ids and the CI popover.
fn row_key(asset: &SessionAsset) -> String {
    asset
        .url
        .clone()
        .or_else(|| {
            asset
                .branch
                .as_ref()
                .map(|b| format!("{}:{b}", asset.project.as_deref().unwrap_or_default()))
        })
        .unwrap_or_else(|| asset.title.clone())
}

/// The pull request's number and state, from the poll when it saw one, else
/// from the PR okena looked up for a registered row.
fn pr_state(asset: &SessionAsset) -> Option<(u32, PrState)> {
    match &asset.state {
        Some(DetectedState::PullRequest { number, state }) => Some((*number, state.clone())),
        _ => asset.pr.as_ref().map(|p| (p.number, p.state.clone())),
    }
}

fn kind_icon(kind: &AgentAssetKind) -> &'static str {
    match kind {
        AgentAssetKind::PullRequest => "icons/git-pull-request.svg",
        AgentAssetKind::Branch => "icons/git-branch.svg",
        AgentAssetKind::Document => "icons/file.svg",
        AgentAssetKind::Task => "icons/bookmark.svg",
        AgentAssetKind::Other => "icons/link.svg",
    }
}

/// Something a row's icon button does.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Action {
    /// Open a link in the browser.
    Open { url: String, label: &'static str },
    /// Put text on the clipboard, and say so.
    Copy {
        text: String,
        label: &'static str,
        done: &'static str,
    },
}

/// The actions a row offers, in the order they are shown, by kind: a PR and
/// a task open their page, a branch copies its name — there is no page for a
/// branch worth leaving okena for, but its name is what you paste next.
fn actions(asset: &SessionAsset) -> Vec<Action> {
    let link = |preferred: Option<&str>| {
        preferred
            .filter(|u| !u.is_empty())
            .map(str::to_string)
            .or_else(|| asset.url.clone().filter(|u| !u.is_empty()))
    };
    let mut out = Vec::new();
    match asset.kind {
        AgentAssetKind::PullRequest => {
            if let Some(url) = link(asset.pr.as_ref().map(|p| p.url.as_str())) {
                out.push(Action::Copy {
                    text: url.clone(),
                    label: "Copy link",
                    done: "Copied the pull request link",
                });
                out.push(Action::Open {
                    url,
                    label: "Open pull request",
                });
            }
        }
        AgentAssetKind::Branch => {
            if let Some(branch) = asset.branch.clone().filter(|b| !b.is_empty()) {
                out.push(Action::Copy {
                    text: branch,
                    label: "Copy branch name",
                    done: "Copied the branch name",
                });
            }
            if let Some(url) = link(None) {
                out.push(Action::Open {
                    url,
                    label: "Open branch",
                });
            }
        }
        AgentAssetKind::Task => {
            if let Some(url) = link(asset.task.as_ref().map(|t| t.url.as_str())) {
                out.push(Action::Copy {
                    text: url.clone(),
                    label: "Copy link",
                    done: "Copied the task link",
                });
                out.push(Action::Open {
                    url,
                    label: "Open task",
                });
            }
        }
        AgentAssetKind::Document | AgentAssetKind::Other => {
            if let Some(url) = link(None) {
                out.push(Action::Open {
                    url,
                    label: "Open link",
                });
            }
        }
    }
    out
}

/// The CI chip, as a popover trigger: a chip that stays lit while its checks
/// are open.
#[derive(IntoElement)]
struct CiTrigger {
    label: String,
    color: u32,
    selected: bool,
}

impl Selectable for CiTrigger {
    fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    fn is_selected(&self) -> bool {
        self.selected
    }
}

impl RenderOnce for CiTrigger {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        div()
            .flex_shrink_0()
            .cursor_pointer()
            .px(px(5.0))
            .py(px(1.0))
            .rounded(px(3.0))
            .bg(with_alpha(
                self.color,
                if self.selected { 0.3 } else { 0.15 },
            ))
            .text_size(ui_text_ms(cx))
            .text_color(rgb(self.color))
            .child(self.label)
    }
}

/// How much a readiness indicator asks of you.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tone {
    Bad,
    Warn,
    Good,
    Muted,
}

/// The readiness indicators a PR row shows: only while the PR is open or a
/// draft. A merged or closed PR has nothing left in its way, and a stale
/// "conflicts" beside "merged" would say otherwise. When okena left readiness
/// out because the repo rejected it recently, the row says so, rather than
/// passing for a clean PR with no reviews.
fn pr_indicators(pr: &PrInfo) -> Vec<(String, Tone)> {
    match (&pr.state, &pr.readiness) {
        (PrState::Open | PrState::Draft, Some(readiness)) => readiness_indicators(readiness),
        (PrState::Open | PrState::Draft, None) if pr.readiness_unavailable => {
            vec![("readiness unavailable".to_string(), Tone::Muted)]
        }
        _ => Vec::new(),
    }
}

/// What stands between a PR and merging it, one compact label each, in the
/// order it is shown. A clean merge says nothing, since it is not in the way.
/// Mergeability GitHub has not worked out yet says so, rather than passing
/// for clean.
fn readiness_indicators(readiness: &PrReadiness) -> Vec<(String, Tone)> {
    let mut out = Vec::new();
    match readiness.merge_state {
        MergeState::Conflicting => out.push(("conflicts".to_string(), Tone::Bad)),
        MergeState::Behind => out.push(("behind base".to_string(), Tone::Warn)),
        MergeState::Unknown => out.push(("mergeable unknown".to_string(), Tone::Muted)),
        MergeState::Clean => {}
    }
    match readiness.review_decision {
        Some(ReviewDecision::ChangesRequested) => {
            out.push(("changes requested".to_string(), Tone::Warn))
        }
        Some(ReviewDecision::Approved) => out.push(("approved".to_string(), Tone::Good)),
        Some(ReviewDecision::ReviewRequired) => {
            out.push(("review required".to_string(), Tone::Muted))
        }
        Some(ReviewDecision::Other) | None => {}
    }
    if readiness.unresolved_threads > 0 {
        let more = if readiness.threads_truncated { "+" } else { "" };
        out.push((
            format!("{}{more} unresolved", readiness.unresolved_threads),
            Tone::Warn,
        ));
    }
    out
}

/// Task, repo and — when the title does not already say it — branch. The link
/// only when there is no repo to name, as the only place it came from. Not
/// the kind: the row's icon already says it.
fn subtitle(asset: &SessionAsset, task_state: Option<&str>) -> String {
    let mut parts = Vec::new();
    parts.extend(task_caption(asset, task_state));
    parts.extend(asset.project.clone());
    if let Some(branch) = asset.branch.as_ref().filter(|b| **b != asset.title) {
        parts.push(branch.clone());
    }
    if asset.project.is_none()
        && let Some(url) = &asset.url
    {
        parts.push(url.clone());
    }
    parts.join(" · ")
}

/// How the row names the task an asset is about, ahead of the repo: its key,
/// and where it now stands on the provider once okena has heard. The state is
/// looked up by the caller, which has the app, so this stays plain.
fn task_caption(asset: &SessionAsset, state: Option<&str>) -> Option<String> {
    let task = asset.task.as_ref()?;
    Some(match state {
        Some(state) => format!("{} · {state}", task.display_key),
        None => task.display_key.clone(),
    })
}

/// "↑2 ↓1" against the base branch (`origin/<default>`), not the upstream, or
/// nothing when neither side has moved.
fn ahead_behind(ahead: Option<usize>, behind: Option<usize>) -> Option<String> {
    let parts: Vec<String> = [
        ahead.filter(|n| *n > 0).map(|n| format!("↑{n}")),
        behind.filter(|n| *n > 0).map(|n| format!("↓{n}")),
    ]
    .into_iter()
    .flatten()
    .collect();
    (!parts.is_empty()).then(|| parts.join(" "))
}

#[cfg(test)]
mod tests {
    // Not `use super::*`: the gpui glob would shadow `#[test]` with
    // `gpui::test`, which expands into itself forever.
    use super::{
        Action, Tone, actions, ahead_behind, pr_indicators, readiness_indicators, subtitle,
    };
    use okena_core::api::{MergeState, PrInfo, PrReadiness, PrState, ReviewDecision};
    use okena_core::harness::AgentAssetKind;
    use okena_core::session_assets::SessionAsset;

    fn asset(title: &str, branch: Option<&str>, project: Option<&str>) -> SessionAsset {
        SessionAsset {
            kind: AgentAssetKind::PullRequest,
            title: title.into(),
            url: Some("https://github.com/o/r/pull/1".into()),
            project: project.map(Into::into),
            branch: branch.map(Into::into),
            state: None,
            uncommitted: None,
            pr: None,
            ci: None,
            task: None,
            registered: true,
        }
    }

    fn readiness(
        merge_state: MergeState,
        review_decision: Option<ReviewDecision>,
        unresolved_threads: usize,
    ) -> PrReadiness {
        PrReadiness {
            merge_state,
            review_decision,
            unresolved_threads,
            threads_truncated: false,
        }
    }

    fn pr(state: PrState, readiness: PrReadiness) -> PrInfo {
        PrInfo {
            url: "https://github.com/o/r/pull/1".into(),
            state,
            number: 1,
            base: None,
            readiness: Some(readiness),
            readiness_unavailable: false,
        }
    }

    #[test]
    fn readiness_left_out_for_the_repo_says_so() {
        let mut open = pr(PrState::Open, readiness(MergeState::Clean, None, 0));
        open.readiness = None;
        assert!(
            pr_indicators(&open).is_empty(),
            "nothing said, nothing shown"
        );
        open.readiness_unavailable = true;
        assert_eq!(
            pr_indicators(&open),
            [("readiness unavailable".to_string(), Tone::Muted)]
        );
        open.state = PrState::Merged;
        assert!(pr_indicators(&open).is_empty());
    }

    #[test]
    fn a_merged_or_closed_pr_shows_no_readiness() {
        let stale = readiness(MergeState::Conflicting, None, 2);
        for state in [PrState::Merged, PrState::Closed] {
            assert!(pr_indicators(&pr(state, stale.clone())).is_empty());
        }
        assert_eq!(
            pr_indicators(&pr(PrState::Draft, stale)).len(),
            2,
            "a draft is still in progress"
        );
    }

    #[test]
    fn a_count_beyond_one_page_is_shown_as_a_floor() {
        let mut many = readiness(MergeState::Clean, None, 100);
        many.threads_truncated = true;
        assert_eq!(labels(&many), ["100+ unresolved"]);
    }

    #[test]
    fn a_review_decision_this_build_does_not_know_shows_nothing() {
        assert!(
            labels(&readiness(
                MergeState::Clean,
                Some(ReviewDecision::Other),
                0
            ))
            .is_empty()
        );
    }

    fn labels(r: &PrReadiness) -> Vec<String> {
        readiness_indicators(r)
            .into_iter()
            .map(|(l, _)| l)
            .collect()
    }

    #[test]
    fn a_conflicting_pr_says_so_first() {
        let indicators = readiness_indicators(&readiness(MergeState::Conflicting, None, 0));
        assert_eq!(indicators, [("conflicts".to_string(), Tone::Bad)]);
    }

    #[test]
    fn mergeability_still_being_computed_is_not_shown_as_clean() {
        assert_eq!(
            labels(&readiness(MergeState::Unknown, None, 0)),
            ["mergeable unknown"]
        );
        assert!(labels(&readiness(MergeState::Clean, None, 0)).is_empty());
    }

    #[test]
    fn unresolved_threads_are_a_count() {
        assert_eq!(
            labels(&readiness(MergeState::Clean, None, 2)),
            ["2 unresolved"]
        );
        assert_eq!(
            labels(&readiness(MergeState::Clean, None, 1)),
            ["1 unresolved"]
        );
    }

    #[test]
    fn every_obstacle_shows_in_order() {
        assert_eq!(
            labels(&readiness(
                MergeState::Behind,
                Some(ReviewDecision::ChangesRequested),
                3
            )),
            ["behind base", "changes requested", "3 unresolved"]
        );
        assert_eq!(
            readiness_indicators(&readiness(
                MergeState::Clean,
                Some(ReviewDecision::Approved),
                0
            )),
            [("approved".to_string(), Tone::Good)]
        );
    }

    #[test]
    fn a_task_is_named_ahead_of_the_repo() {
        let mut row = asset("Detect assets", None, Some("okena"));
        row.task = Some(okena_core::tasks::TaskRef {
            id: okena_core::tasks::TaskId::new("linear", "u1"),
            display_key: "QBL-374".into(),
            title: "t".into(),
            url: "http://x".into(),
            parent_id: None,
            parent_key: None,
        });
        assert_eq!(subtitle(&row, None), "QBL-374 · okena");
        // Once okena has heard where the task stands, it follows the key.
        assert_eq!(
            subtitle(&row, Some("In Progress")),
            "QBL-374 · In Progress · okena"
        );
    }

    #[test]
    fn a_state_without_a_task_says_nothing() {
        assert_eq!(
            subtitle(&asset("Detect assets", None, Some("okena")), Some("Done")),
            "okena"
        );
    }

    #[test]
    fn the_branch_is_named_only_when_the_title_does_not_already() {
        assert_eq!(
            subtitle(&asset("feat/x", Some("feat/x"), Some("okena")), None),
            "okena"
        );
        assert_eq!(
            subtitle(&asset("Detect assets", Some("feat/x"), Some("okena")), None),
            "okena · feat/x"
        );
    }

    #[test]
    fn the_link_stands_in_for_a_missing_repo() {
        assert_eq!(
            subtitle(&asset("Detect assets", None, None), None),
            "https://github.com/o/r/pull/1"
        );
    }

    #[test]
    fn ahead_behind_skips_sides_that_have_not_moved() {
        assert_eq!(ahead_behind(Some(2), Some(1)).as_deref(), Some("↑2 ↓1"));
        assert_eq!(ahead_behind(Some(0), Some(3)).as_deref(), Some("↓3"));
        assert_eq!(ahead_behind(Some(0), None), None);
    }

    #[test]
    fn a_pr_offers_its_page_and_its_link() {
        let row = asset("Detect assets", None, Some("okena"));
        assert_eq!(
            actions(&row),
            [
                Action::Copy {
                    text: "https://github.com/o/r/pull/1".into(),
                    label: "Copy link",
                    done: "Copied the pull request link",
                },
                Action::Open {
                    url: "https://github.com/o/r/pull/1".into(),
                    label: "Open pull request",
                },
            ]
        );
    }

    #[test]
    fn a_branch_copies_its_name_and_has_no_page_without_a_link() {
        let mut row = asset("feat/x", Some("feat/x"), Some("okena"));
        row.kind = AgentAssetKind::Branch;
        row.url = None;
        assert_eq!(
            actions(&row),
            [Action::Copy {
                text: "feat/x".into(),
                label: "Copy branch name",
                done: "Copied the branch name",
            }]
        );
    }

    #[test]
    fn a_task_opens_its_own_url_before_the_rows() {
        let mut row = asset("Fix it", None, None);
        row.kind = AgentAssetKind::Task;
        row.url = Some("https://example.com/other".into());
        row.task = Some(okena_core::tasks::TaskRef {
            id: okena_core::tasks::TaskId::new("linear", "u1"),
            display_key: "QBL-1".into(),
            title: "Fix it".into(),
            url: "https://linear.app/x/issue/QBL-1".into(),
            parent_id: None,
            parent_key: None,
        });
        assert!(actions(&row).contains(&Action::Open {
            url: "https://linear.app/x/issue/QBL-1".into(),
            label: "Open task",
        }));
    }

    #[test]
    fn nothing_to_open_offers_nothing() {
        let mut row = asset("Notes", None, None);
        row.kind = AgentAssetKind::Document;
        row.url = None;
        assert!(actions(&row).is_empty());
    }
}
