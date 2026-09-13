//! One row of a session's PRODUCED list.
//!
//! Shared by the session's own panel and the task detail, which list the same
//! rows and had drifted into two copies of the same markup.
//!
//! A PR row says whether it can merge: conflicts, the CI rollup, the review
//! decision and unresolved threads, each a compact chip. The CI chip opens the
//! same checks list the project header's CI popover shows.

use gpui::prelude::*;
use gpui::*;
use gpui_component::popover::Popover;
use gpui_component::{Selectable, h_flex, v_flex};
use okena_core::api::{MergeState, PrInfo, PrReadiness, PrState, ReviewDecision};
use okena_core::session_assets::{DetectedState, SessionAsset};
use okena_views_git::git_header::{render_ci_checks_header, render_ci_checks_list};

use super::worktree_card::{PushState, chip, ci_chip_style, pr_chip_style};
use crate::theme::{theme, with_alpha};
use crate::ui::tokens::ui_text_ms;

/// Render one produced asset on `bg`. The title opens the row's link.
pub fn render_asset_row(asset: &SessionAsset, bg: u32, cx: &App) -> AnyElement {
    let t = theme(cx);

    let mut chips: Vec<AnyElement> = Vec::new();
    match &asset.state {
        Some(DetectedState::LocalOnly) => chips.push(chip("local only".into(), t.warning, cx)),
        Some(DetectedState::Pushed {
            unpushed,
            ahead,
            behind,
        }) => {
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
        Some(DetectedState::PullRequest { number, state }) => {
            let (color, label) = pr_chip_style(*number, state, &t);
            let pr_chip = chip(label, color, cx);
            // The PR chip opens the PR, like the title does.
            chips.push(match asset.url.clone() {
                Some(url) => div()
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, move |_, _window, cx| cx.open_url(&url))
                    .child(pr_chip)
                    .into_any_element(),
                None => pr_chip,
            });
        }
        None => {}
    }
    if let Some(pr) = asset.pr.as_ref() {
        for (label, tone) in pr_indicators(pr) {
            let color = match tone {
                Tone::Bad => t.error,
                Tone::Warn => t.warning,
                Tone::Good => t.success,
                Tone::Muted => t.text_muted,
            };
            chips.push(chip(label, color, cx));
        }
    }
    if let Some(summary) = asset.ci.clone() {
        let (color, label) = ci_chip_style(
            &summary.status,
            summary.passed,
            summary.failed,
            summary.pending,
            &t,
        );
        let pr = asset.pr.clone();
        let key = asset
            .url
            .clone()
            .or_else(|| {
                asset
                    .branch
                    .as_ref()
                    .map(|b| format!("{}:{b}", asset.project.as_deref().unwrap_or_default()))
            })
            .unwrap_or_else(|| asset.title.clone());
        chips.push(
            Popover::new(SharedString::from(format!("asset-ci-{key}")))
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
                .into_any_element(),
        );
    }
    if let Some(changes) = asset.uncommitted {
        chips.push(chip(
            format!("+{} −{} uncommitted", changes.added, changes.removed),
            t.text_muted,
            cx,
        ));
    }

    // Only the words open the link: the chips below hold a popover of their
    // own, and a click on the CI chip must not also leave for GitHub.
    let heading = v_flex()
        .w_full()
        .min_w_0()
        .gap(px(2.0))
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
                .child(subtitle(
                    asset,
                    crate::views::known_tasks::task_state(asset.task.as_ref(), cx).as_deref(),
                )),
        )
        .when_some(asset.url.clone(), |heading, url| {
            heading
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, move |_, _window, cx| cx.open_url(&url))
        });

    v_flex()
        .w_full()
        .min_w_0()
        .gap(px(2.0))
        .px(px(8.0))
        .py(px(5.0))
        .rounded(px(4.0))
        .bg(rgb(bg))
        .border_1()
        .border_color(rgb(t.border))
        .child(heading)
        .when(!chips.is_empty(), |row| {
            row.child(h_flex().gap(px(4.0)).flex_wrap().children(chips))
        })
        .into_any_element()
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
/// "conflicts" beside "merged" would say otherwise.
fn pr_indicators(pr: &PrInfo) -> Vec<(String, Tone)> {
    match (&pr.state, &pr.readiness) {
        (PrState::Open | PrState::Draft, Some(readiness)) => readiness_indicators(readiness),
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

/// Kind, repo and — when the title does not already say it — branch. The link
/// only when there is no repo to name, as the only place it came from.
fn subtitle(asset: &SessionAsset, task_state: Option<&str>) -> String {
    let mut parts = vec![asset.kind.label().to_string()];
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
    use super::{Tone, ahead_behind, pr_indicators, readiness_indicators, subtitle};
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
        }
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
        assert_eq!(subtitle(&row, None), "PR · QBL-374 · okena");
        // Once okena has heard where the task stands, it follows the key.
        assert_eq!(
            subtitle(&row, Some("In Progress")),
            "PR · QBL-374 · In Progress · okena"
        );
    }

    #[test]
    fn a_state_without_a_task_says_nothing() {
        assert_eq!(
            subtitle(&asset("Detect assets", None, Some("okena")), Some("Done")),
            "PR · okena"
        );
    }

    #[test]
    fn the_branch_is_named_only_when_the_title_does_not_already() {
        assert_eq!(
            subtitle(&asset("feat/x", Some("feat/x"), Some("okena")), None),
            "PR · okena"
        );
        assert_eq!(
            subtitle(&asset("Detect assets", Some("feat/x"), Some("okena")), None),
            "PR · okena · feat/x"
        );
    }

    #[test]
    fn the_link_stands_in_for_a_missing_repo() {
        assert_eq!(
            subtitle(&asset("Detect assets", None, None), None),
            "PR · https://github.com/o/r/pull/1"
        );
    }

    #[test]
    fn ahead_behind_skips_sides_that_have_not_moved() {
        assert_eq!(ahead_behind(Some(2), Some(1)).as_deref(), Some("↑2 ↓1"));
        assert_eq!(ahead_behind(Some(0), Some(3)).as_deref(), Some("↓3"));
        assert_eq!(ahead_behind(Some(0), None), None);
    }
}
