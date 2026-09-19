//! The chosen agents' usage, side by side on the status bar.

use crate::selection::{Agent, selected};
use gpui::*;
use gpui_component::h_flex;
use okena_extensions::ExtensionSettingsStore;
use std::collections::HashMap;

pub struct UsageBar {
    /// Each chosen agent's widget. Dropping one stops its poll once no other
    /// window shows it.
    widgets: HashMap<Agent, AnyView>,
}

impl UsageBar {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            widgets: HashMap::new(),
        };
        this.sync(cx);
        // Choosing agents in settings adds and drops them here.
        cx.observe_global::<ExtensionSettingsStore>(|this, cx| this.sync(cx))
            .detach();
        this
    }

    fn sync(&mut self, cx: &mut Context<Self>) {
        let chosen = selected(cx);
        self.widgets.retain(|agent, _| chosen.contains(agent));
        for agent in chosen {
            self.widgets.entry(agent).or_insert_with(|| match agent {
                Agent::Claude => cx.new(crate::claude::ClaudeUsage::new).into(),
                Agent::Codex => cx.new(crate::codex::CodexUsage::new).into(),
                Agent::Copilot => cx.new(crate::copilot::CopilotUsage::new).into(),
            });
        }
        cx.notify();
    }
}

impl Render for UsageBar {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        h_flex().gap(px(6.0)).items_center().children(
            Agent::ALL
                .into_iter()
                .filter_map(|agent| self.widgets.get(&agent).cloned()),
        )
    }
}

/// The agent's icon, leading its figures so three agents' bars side by side
/// say whose is whose.
pub fn agent_icon(agent: Agent, color: u32) -> Svg {
    svg()
        .path(agent.icon())
        .flex_shrink_0()
        .size(px(11.0))
        .text_color(rgb(color))
}
