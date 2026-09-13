//! One row of a session's PRODUCED list.
//!
//! Shared by the session's own panel and the task detail, which list the same
//! rows and had drifted into two copies of the same markup.

use gpui::prelude::*;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_core::session_assets::{DetectedState, SessionAsset};

use super::worktree_card::{PushState, chip, pr_chip_style};
use crate::theme::theme;
use crate::ui::tokens::ui_text_ms;

/// Render one produced asset on `bg`. A row with a link opens it.
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
            chips.push(chip(label, color, cx));
        }
        None => {}
    }
    if let Some(changes) = asset.uncommitted {
        chips.push(chip(
            format!("+{} −{} uncommitted", changes.added, changes.removed),
            t.text_muted,
            cx,
        ));
    }

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
                .child(subtitle(asset)),
        )
        .when(!chips.is_empty(), |row| {
            row.child(h_flex().gap(px(4.0)).flex_wrap().children(chips))
        })
        .when_some(asset.url.clone(), |row, url| {
            row.cursor_pointer()
                .on_mouse_down(MouseButton::Left, move |_, _window, cx| cx.open_url(&url))
        })
        .into_any_element()
}

/// Kind, repo and — when the title does not already say it — branch. The link
/// only when there is no repo to name, as the only place it came from.
fn subtitle(asset: &SessionAsset) -> String {
    let mut parts = vec![asset.kind.label().to_string()];
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

/// "↑2 ↓1" against the review base, or nothing when neither side has moved.
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
    use super::{ahead_behind, subtitle};
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
            registered: true,
        }
    }

    #[test]
    fn the_branch_is_named_only_when_the_title_does_not_already() {
        assert_eq!(
            subtitle(&asset("feat/x", Some("feat/x"), Some("okena"))),
            "PR · okena"
        );
        assert_eq!(
            subtitle(&asset("Detect assets", Some("feat/x"), Some("okena"))),
            "PR · okena · feat/x"
        );
    }

    #[test]
    fn the_link_stands_in_for_a_missing_repo() {
        assert_eq!(
            subtitle(&asset("Detect assets", None, None)),
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
