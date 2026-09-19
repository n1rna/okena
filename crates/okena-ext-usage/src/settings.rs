//! Settings → Usage: which agents' usage shows on the bar, where Claude keeps
//! its login, and the working days the weekly bars pace against.

use crate::selection::{Agent, EXTENSION_ID, selected, set_selected};
use gpui::*;
use okena_extensions::ExtensionSettingsStore;
use okena_ui::settings::{
    input_box, section_container, section_header, section_note, settings_input_row,
    settings_row_with_desc,
};
use okena_ui::simple_input::{InputChangedEvent, SimpleInput, SimpleInputState};
use okena_ui::toggle::toggle_switch;
use okena_ui::tokens::ui_text_md;
use okena_usage::WorkingDaysSetting;

/// Where Claude's config directory is kept. Under Claude's own namespace
/// rather than this extension's: the daemon reads it too, to start Claude
/// agents against the same login.
const CLAUDE_NAMESPACE: &str = "claude-code";

fn claude_config_dir(cx: &App) -> String {
    cx.global::<ExtensionSettingsStore>()
        .get(CLAUDE_NAMESPACE, cx)
        .and_then(|settings| settings["config_dir"].as_str().map(ToOwned::to_owned))
        .unwrap_or_default()
}

pub struct UsageSettingsView {
    config_dir_input: Entity<SimpleInputState>,
    working_days: Entity<WorkingDaysSetting>,
}

impl UsageSettingsView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let current = claude_config_dir(cx);
        let config_dir_input = cx.new(|cx| {
            SimpleInputState::new(cx)
                .placeholder("e.g. ~/.claude-work")
                .default_value(current)
        });
        cx.observe_global::<ExtensionSettingsStore>(|this, cx| {
            let next = claude_config_dir(cx);
            this.config_dir_input.update(cx, |input, cx| {
                if input.value() != next {
                    input.set_value(next, cx);
                }
            });
            cx.notify();
        })
        .detach();
        cx.subscribe(&config_dir_input, |_this, entity, _: &InputChangedEvent, cx| {
            let value = entity.read(cx).value().trim().to_string();
            let settings = if value.is_empty() {
                serde_json::json!({})
            } else {
                serde_json::json!({ "config_dir": value })
            };
            ExtensionSettingsStore::update(CLAUDE_NAMESPACE, settings, cx);
        })
        .detach();
        Self {
            config_dir_input,
            working_days: cx.new(WorkingDaysSetting::new),
        }
    }
}

impl Render for UsageSettingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = okena_extensions::theme(cx);
        let chosen = selected(cx);
        let mut agents = section_container(&t);
        let count = Agent::ALL.len();
        for (index, agent) in Agent::ALL.into_iter().enumerate() {
            let on = chosen.contains(&agent);
            agents = agents.child(
                settings_row_with_desc(
                    format!("{EXTENSION_ID}-{}", agent.slug()),
                    agent.label(),
                    agent.description(),
                    &t,
                    cx,
                    index + 1 < count,
                )
                .child(
                    toggle_switch(format!("{EXTENSION_ID}-{}-toggle", agent.slug()), on, &t)
                        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                            set_selected(agent, !on, cx);
                        }),
                ),
            );
        }
        div()
            .flex()
            .flex_col()
            .child(section_header("Shown on the status bar", &t, cx))
            .child(section_note(
                "Each agent's limits, read with the login its CLI already has. Hover one for the detail.",
                &t,
                cx,
            ))
            .child(agents)
            .child(section_header("Claude", &t, cx))
            .child(
                section_container(&t).child(
                    settings_input_row(
                        "claude-config-dir",
                        "Config directory",
                        "Optional override for Claude credentials/config. Supports ~/. Falls back to CLAUDE_CONFIG_DIR, then ~/.claude.",
                        &t,
                        cx,
                        false,
                    )
                    .child(
                        input_box(&t)
                            .child(SimpleInput::new(&self.config_dir_input).text_size(ui_text_md(cx))),
                    ),
                ),
            )
            .child(self.working_days.clone())
    }
}
