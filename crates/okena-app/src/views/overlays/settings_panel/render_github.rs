//! GitHub settings — the GitHub Enterprise hosts okena treats as GitHub.
//!
//! github.com, GHE.com tenants, the hosts gh is logged into and `GH_HOST` are
//! recognised on their own; this list adds hosts known in none of those ways.
//! Tokens still come from env vars or `gh auth login`.

use super::SettingsPanel;
use super::components::{section_container, section_header};
use super::render_specs::muted_row;
use crate::settings::settings_entity;
use crate::theme::theme;
use crate::ui::tokens::{ui_text, ui_text_ms};
use crate::views::components::SimpleInput;
use gpui::prelude::*;
use gpui::*;
use gpui_component::h_flex;

impl SettingsPanel {
    fn add_github_host(&mut self, cx: &mut Context<Self>) {
        let value = self.github_host_input.read(cx).value().trim().to_string();
        if value.is_empty() {
            return;
        }
        let mut hosts = settings_entity(cx)
            .read(cx)
            .settings
            .github_enterprise_hosts
            .clone();
        hosts.push(value);
        settings_entity(cx).update(cx, |state, cx| state.set_github_enterprise_hosts(hosts, cx));
        self.github_host_input.update(cx, |i, cx| i.set_value("", cx));
    }

    fn remove_github_host(&mut self, index: usize, cx: &mut Context<Self>) {
        let mut hosts = settings_entity(cx)
            .read(cx)
            .settings
            .github_enterprise_hosts
            .clone();
        if index < hosts.len() {
            hosts.remove(index);
        }
        settings_entity(cx).update(cx, |state, cx| state.set_github_enterprise_hosts(hosts, cx));
    }

    fn github_button(
        &self,
        id: String,
        label: &str,
        primary: bool,
        on_click: impl Fn(&mut Self, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        div()
            .id(SharedString::from(id))
            .cursor_pointer()
            .flex_shrink_0()
            .px(px(10.0))
            .py(px(3.0))
            .rounded(px(4.0))
            .when(primary, |d| {
                d.bg(rgb(t.button_primary_bg))
                    .hover(|s| s.bg(rgb(t.button_primary_hover)))
                    .text_color(rgb(t.button_primary_fg))
            })
            .when(!primary, |d| {
                d.border_1()
                    .border_color(rgb(t.border))
                    .hover(|s| s.bg(rgb(t.bg_hover)))
                    .text_color(rgb(t.text_secondary))
            })
            .text_size(ui_text_ms(cx))
            .child(label.to_string())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _window, cx| on_click(this, cx)),
            )
            .into_any_element()
    }

    pub(super) fn render_github(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let hosts = settings_entity(cx)
            .read(cx)
            .settings
            .github_enterprise_hosts
            .clone();

        let mut container = section_container(&t).child(muted_row(
            "github.com, GHE.com tenants (*.ghe.com), hosts gh is logged into and GH_HOST are \
             recognised already. Add a GitHub Enterprise Server host gh doesn't know, e.g. \
             github.acme.corp, to get PR and CI status for its repos. Tokens come from \
             GH_ENTERPRISE_TOKEN or `gh auth login --hostname <host>`.",
            &t,
            cx,
        ));
        for (index, host) in hosts.iter().enumerate() {
            container = container.child(
                h_flex()
                    .items_center()
                    .gap(px(12.0))
                    .px(px(12.0))
                    .py(px(8.0))
                    .border_t_1()
                    .border_color(rgb(t.border))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(ui_text(13.0, cx))
                            .text_color(rgb(t.text_primary))
                            .child(host.clone()),
                    )
                    .child(self.github_button(
                        format!("github-host-remove-{index}"),
                        "Remove",
                        false,
                        move |this, cx| this.remove_github_host(index, cx),
                        cx,
                    )),
            );
        }
        container = container.child(
            h_flex()
                .gap(px(8.0))
                .items_center()
                .px(px(12.0))
                .py(px(8.0))
                .border_t_1()
                .border_color(rgb(t.border))
                .child(
                    okena_ui::input::input_container(&t, None)
                        .flex_1()
                        .min_w_0()
                        .px(px(8.0))
                        .py(px(5.0))
                        .child(
                            SimpleInput::new(&self.github_host_input).text_size(ui_text(13.0, cx)),
                        ),
                )
                .child(self.github_button(
                    "github-host-add".into(),
                    "Add host",
                    true,
                    |this, cx| this.add_github_host(cx),
                    cx,
                )),
        );

        div()
            .child(section_header("Enterprise hosts", &t, cx))
            .child(container)
    }
}
