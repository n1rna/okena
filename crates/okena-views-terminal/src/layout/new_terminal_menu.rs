//! What a split or a new tab runs: a shell, or one of the coding agents.
//!
//! Hovering a split or new-tab button opens a small strip of icons under it,
//! one per kind of session. Clicking an icon opens the pane on that kind and
//! makes it the default; clicking the button itself opens the default, so the
//! common case stays one click.

use std::time::Duration;

use crate::layout::layout_container::{LayoutContainer, NewTerminalMenu, NewTerminalTarget};
use gpui::prelude::*;
use gpui::*;
use gpui_component::h_flex;
use gpui_component::tooltip::Tooltip;
use okena_core::agents::{AGENT_COMMANDS, display_name};
use okena_terminal::shell_config::ShellType;
use okena_ui::theme::theme;

use crate::ActionDispatch;

/// Where the last choice is kept, in the settings' extension map.
const SETTINGS_ID: &str = "new-pane";

/// Long enough that sweeping the pointer across the header opens nothing.
const OPEN_DELAY: Duration = Duration::from_millis(120);
/// Long enough to cross the gap between a button and its strip.
const CLOSE_DELAY: Duration = Duration::from_millis(220);

const ITEM_SIZE: f32 = 22.0;
const ITEM_GAP: f32 = 2.0;
const STRIP_PADDING: f32 = 3.0;

/// The agent a pane opens on, or `None` for the project's shell.
pub(super) type PaneChoice = Option<&'static str>;

/// Every kind of session a pane can open on, in strip order.
fn choices() -> impl Iterator<Item = PaneChoice> {
    std::iter::once(None).chain(AGENT_COMMANDS.iter().copied().map(Some))
}

fn choice_icon(choice: PaneChoice) -> &'static str {
    match choice {
        None => "icons/terminal.svg",
        Some("claude") => "icons/agent-claude.svg",
        Some("copilot") => "icons/agent-copilot.svg",
        Some("codex") => "icons/agent-codex.svg",
        Some(_) => "icons/bot.svg",
    }
}

fn choice_label(choice: PaneChoice) -> String {
    match choice {
        None => "Shell".to_string(),
        Some(command) => display_name(command),
    }
}

/// The shell to ask the daemon for.
fn choice_shell(choice: PaneChoice) -> Option<ShellType> {
    // A bare custom shell: no args, so the agent starts interactively rather
    // than running a one-shot prompt.
    choice.map(|command| ShellType::Custom {
        path: command.to_string(),
        args: Vec::new(),
    })
}

/// The choice a stored value names. Anything okena no longer offers falls back
/// to the shell rather than opening a pane on a command it cannot recognise.
fn choice_from_stored(value: Option<&serde_json::Value>) -> PaneChoice {
    let stored = value?.get("default")?.as_str()?;
    AGENT_COMMANDS
        .iter()
        .copied()
        .find(|command| *command == stored)
}

/// What a plain click on a split or new-tab button opens: the last choice made.
pub(super) fn default_choice(cx: &App) -> PaneChoice {
    let store = cx.try_global::<okena_extensions::ExtensionSettingsStore>()?;
    choice_from_stored(store.get(SETTINGS_ID, cx).as_ref())
}

fn remember_choice(choice: PaneChoice, cx: &mut App) {
    if default_choice(cx) == choice
        || cx
            .try_global::<okena_extensions::ExtensionSettingsStore>()
            .is_none()
    {
        return;
    }
    okena_extensions::ExtensionSettingsStore::update(
        SETTINGS_ID,
        serde_json::json!({ "default": choice }),
        cx,
    );
}

fn strip_width(items: usize) -> f32 {
    let items = items as f32;
    // Padding on both sides, plus the border.
    items * ITEM_SIZE + (items - 1.0).max(0.0) * ITEM_GAP + 2.0 * STRIP_PADDING + 2.0
}

impl<D: ActionDispatch + Send + Sync + 'static> LayoutContainer<D> {
    /// The pointer entered or left one of the buttons.
    pub(super) fn hover_new_terminal_button(
        &mut self,
        target: NewTerminalTarget,
        layout_path: Vec<usize>,
        hovered: bool,
        cx: &mut Context<Self>,
    ) {
        if !hovered {
            self.close_new_terminal_menu_soon(cx);
            return;
        }
        let token = self.next_new_terminal_hover();
        let menu = NewTerminalMenu {
            target,
            layout_path,
        };
        // Already open for a neighbouring button: follow the pointer at once.
        if self.new_terminal_menu.is_some() {
            self.new_terminal_menu = Some(menu);
            cx.notify();
            return;
        }
        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            cx.background_executor().timer(OPEN_DELAY).await;
            let _ = this.update(cx, |this, cx| {
                if this.new_terminal_hover == token {
                    this.new_terminal_menu = Some(menu);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// The pointer entered or left the strip itself.
    fn hover_new_terminal_strip(&mut self, hovered: bool, cx: &mut Context<Self>) {
        if hovered {
            // Cancels the close the button scheduled when the pointer left it.
            self.next_new_terminal_hover();
        } else {
            self.close_new_terminal_menu_soon(cx);
        }
    }

    fn next_new_terminal_hover(&mut self) -> u64 {
        self.new_terminal_hover += 1;
        self.new_terminal_hover
    }

    fn close_new_terminal_menu_soon(&mut self, cx: &mut Context<Self>) {
        let token = self.next_new_terminal_hover();
        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            cx.background_executor().timer(CLOSE_DELAY).await;
            let _ = this.update(cx, |this, cx| {
                if this.new_terminal_hover == token && this.new_terminal_menu.take().is_some() {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Open the pane `menu` describes on `choice`, and close the strip.
    pub(super) fn create_new_terminal(
        &mut self,
        menu: NewTerminalMenu,
        choice: PaneChoice,
        cx: &mut Context<Self>,
    ) {
        self.next_new_terminal_hover();
        self.new_terminal_menu = None;
        cx.notify();
        let Some(dispatcher) = self.action_dispatcher.clone() else {
            return;
        };
        let project_id = self.project_id.clone();
        let shell_type = choice_shell(choice);
        let request = match menu.target {
            NewTerminalTarget::Split(direction) => okena_core::api::ActionRequest::SplitTerminal {
                project_id,
                path: menu.layout_path,
                direction,
                shell_type,
            },
            NewTerminalTarget::Tab { in_group } => okena_core::api::ActionRequest::AddTab {
                project_id,
                path: menu.layout_path,
                in_group,
                shell_type,
            },
        };
        dispatcher.dispatch(request, cx);
    }

    /// The strip for the open menu, anchored under the button it belongs to.
    pub(super) fn render_new_terminal_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.new_terminal_menu.clone()?;
        let anchor = *self.new_terminal_anchors.get(menu.target.button_index())?;
        let t = theme(cx);
        let default = default_choice(cx);

        let items: Vec<AnyElement> = choices()
            .map(|choice| {
                let is_default = choice == default;
                let label = choice_label(choice);
                let tooltip = if is_default {
                    format!("{label} (default)")
                } else {
                    label.clone()
                };
                let menu = menu.clone();
                div()
                    .id(SharedString::from(format!("new-pane-{label}")))
                    .cursor_pointer()
                    .w(px(ITEM_SIZE))
                    .h(px(ITEM_SIZE))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.0))
                    .when(is_default, |d| d.bg(rgb(t.bg_selection)))
                    .hover(|s| s.bg(rgb(t.bg_hover)))
                    .child(
                        svg()
                            .path(choice_icon(choice))
                            .size(px(13.0))
                            .text_color(rgb(if is_default {
                                t.text_primary
                            } else {
                                t.text_secondary
                            })),
                    )
                    .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, _window, cx| {
                        remember_choice(choice, cx);
                        this.create_new_terminal(menu.clone(), choice, cx);
                    }))
                    .into_any_element()
            })
            .collect();

        // Centred under the button; `snap_to_window` keeps it on screen for
        // the buttons at the window's edge.
        let width = strip_width(items.len());
        let position = point(
            anchor.origin.x + anchor.size.width / 2.0 - px(width / 2.0),
            anchor.origin.y + anchor.size.height + px(2.0),
        );

        Some(
            deferred(
                anchored()
                    .position(position)
                    .anchor(Anchor::TopLeft)
                    .snap_to_window()
                    .child(
                        h_flex()
                            .id("new-pane-strip")
                            .occlude()
                            .gap(px(ITEM_GAP))
                            .p(px(STRIP_PADDING))
                            .rounded(px(6.0))
                            .border_1()
                            .border_color(rgb(t.border))
                            .bg(rgb(t.bg_secondary))
                            .shadow_md()
                            .on_hover(cx.listener(|this, hovered: &bool, _window, cx| {
                                this.hover_new_terminal_strip(*hovered, cx);
                            }))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .children(items),
                    ),
            )
            .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    // Named imports: `super::*` would bring in `gpui::test` over `#[test]`.
    use super::{
        AGENT_COMMANDS, ITEM_GAP, ITEM_SIZE, PaneChoice, STRIP_PADDING, ShellType,
        choice_from_stored, choice_icon, choice_label, choice_shell, choices, strip_width,
    };

    #[test]
    fn the_strip_offers_the_shell_first_then_every_known_agent() {
        let all: Vec<PaneChoice> = choices().collect();
        assert_eq!(all.first(), Some(&None));
        assert_eq!(all.len(), AGENT_COMMANDS.len() + 1);
        for command in AGENT_COMMANDS {
            assert!(all.contains(&Some(*command)));
        }
    }

    #[test]
    fn every_choice_has_its_own_icon_and_name() {
        let icons: std::collections::HashSet<_> = choices().map(choice_icon).collect();
        let labels: std::collections::HashSet<_> = choices().map(choice_label).collect();
        assert_eq!(icons.len(), choices().count());
        assert_eq!(labels.len(), choices().count());
        assert!(
            !icons.contains("icons/bot.svg"),
            "a known agent has no icon"
        );
    }

    #[test]
    fn the_shell_asks_for_the_project_default_and_an_agent_for_its_command() {
        assert_eq!(choice_shell(None), None);
        assert_eq!(
            choice_shell(Some("claude")),
            Some(ShellType::Custom {
                path: "claude".to_string(),
                args: Vec::new(),
            })
        );
    }

    #[test]
    fn the_last_choice_is_read_back_and_an_unknown_one_falls_back_to_the_shell() {
        let stored = |value: serde_json::Value| choice_from_stored(Some(&value));
        assert_eq!(
            stored(serde_json::json!({ "default": "claude" })),
            Some("claude")
        );
        assert_eq!(stored(serde_json::json!({ "default": null })), None);
        assert_eq!(stored(serde_json::json!({ "default": "gone" })), None);
        assert_eq!(stored(serde_json::json!("claude")), None);
        assert_eq!(choice_from_stored(None), None);
    }

    #[test]
    fn the_strip_is_wide_enough_for_its_icons() {
        assert_eq!(strip_width(1), ITEM_SIZE + 2.0 * STRIP_PADDING + 2.0);
        assert_eq!(
            strip_width(4),
            4.0 * ITEM_SIZE + 3.0 * ITEM_GAP + 2.0 * STRIP_PADDING + 2.0
        );
    }
}
