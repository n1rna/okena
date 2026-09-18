//! The one control for starting an agent session, wherever one can be started.
//!
//! Starting an agent used to take a different shape in every place that could
//! do it: a "Start work" button, a "Break down with agent" button, agent chips
//! above a "Start session" button, and an "Open session" button standing in for
//! all of them once something was running. The launcher is the single shape:
//! what would start and with which brief, a pill per agent — its name to start
//! it right away, its model to change for this launch — a way to configure it
//! first, and — once something is running — the sessions
//! themselves, with how they are doing and a way in.
//!
//! It knows nothing about tasks, specs or agents in particular. The caller
//! says what the options are and what clicking them does.

use crate::theme::{theme, with_alpha};
use crate::tokens::{ui_text_md, ui_text_ms};
use gpui::prelude::*;
use gpui::*;
use gpui_component::tooltip::Tooltip;
use gpui_component::{h_flex, v_flex};
use okena_core::agent_model::{self, AgentModels};
use std::collections::HashMap;
use std::rc::Rc;

/// One way to start: an agent, or deliberately none.
#[derive(Clone, Debug)]
pub struct LaunchOption {
    /// Command handed to the daemon. Empty means start no agent — a plain
    /// shell, worktrees only, a scaffold — which each caller names in `label`.
    pub command: SharedString,
    pub label: SharedString,
    pub icon: SharedString,
    pub accent: u32,
}

impl LaunchOption {
    fn tooltip(&self) -> SharedString {
        if self.command.is_empty() {
            self.label.clone()
        } else {
            format!("Start with {}", self.label).into()
        }
    }
}

/// The brief a launch starts with, as the launcher names it.
///
/// Every launcher that briefs its agent says which template it uses and which
/// model that runs on, so a break-down on a cheaper model than the task it
/// breaks down is something a person can see before starting it.
#[derive(Clone, Debug, PartialEq)]
pub struct LaunchBrief {
    /// The template's name, e.g. `task-start`.
    pub name: SharedString,
    /// Where the template comes from, for its tooltip.
    pub source: SharedString,
    /// The models the template names; which one a CLI runs is decided by
    /// [`okena_core::agent_model`].
    pub models: AgentModels,
}

/// What was launched: the option's command, and the model picked for this
/// launch — `None` when nothing was picked and the template decides,
/// `Some("")` for the CLI's own default.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launch {
    pub command: SharedString,
    pub model: Option<String>,
}

impl Launch {
    /// The model this launch runs `models` on: the pick, else the template's.
    /// `None` runs the CLI's default.
    pub fn resolve(&self, models: &AgentModels) -> Option<String> {
        agent_model::resolve(models, &self.command, self.model.as_deref())
    }
}

/// What a launcher remembers while it is open: the agent chosen, the model
/// picked per agent, and which menu is open. Dropped when the launcher stops
/// rendering, so a choice lasts until it closes.
#[derive(Default)]
struct LauncherState {
    /// The option chosen in the agent menu; `None` until one is, which means
    /// the default.
    chosen: Option<SharedString>,
    /// Keyed by command; `""` is "the CLI's default".
    picks: HashMap<SharedString, String>,
    menu: Option<Menu>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Menu {
    Agent,
    Model,
}

/// What the pill's model side says: the model this launch runs the agent
/// on, `default` when none resolves. `None` — no model part at all — for a
/// launcher with no brief, and for an agent okena passes no model to.
fn model_label(brief: Option<&LaunchBrief>, command: &str, picked: Option<&str>) -> Option<String> {
    let brief = brief?;
    if command.is_empty() || agent_model::flag(command).is_none() {
        return None;
    }
    Some(
        agent_model::resolve(&brief.models, command, picked)
            .unwrap_or_else(|| "default".to_string()),
    )
}

/// One way of launching, when a launcher has more than one.
///
/// Not another agent to pick — the agents are the pills below. This
/// is what pressing one of them will *do*: start a single session, or fan out,
/// or hand the decision to the agent. It belongs inside the card because it
/// changes what that card's buttons mean, and a control that changes a
/// button's meaning while sitting outside it is a control people press by
/// accident.
#[derive(Clone)]
pub struct LaunchMode {
    /// Handed back to `on_mode`.
    pub id: SharedString,
    pub label: SharedString,
    pub selected: bool,
}

/// A session already running for whatever the launcher starts.
#[derive(Clone, Debug)]
pub struct LauncherSession {
    /// Handed back to `on_open`.
    pub id: SharedString,
    pub name: SharedString,
    /// "running", "waiting · 3m", "stopped" — pre-worded by the caller, which
    /// knows how it decided.
    pub activity: SharedString,
    pub activity_color: u32,
    /// The agent in it, when one is recognized: its icon and accent.
    pub agent: Option<LaunchOption>,
    /// What the agent last said it was doing.
    pub status: Option<SharedString>,
    /// Something wrong worth a word, e.g. that okena's MCP is not wired in.
    pub warning: Option<SharedString>,
}

/// How much chrome the launcher draws.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LauncherStyle {
    /// A card of its own, for a launcher standing among other content.
    #[default]
    Card,
    /// No border or fill, for a form footer that already frames it.
    Inline,
}

type CommandHandler = Rc<dyn Fn(&SharedString, &mut Window, &mut App)>;
type LaunchHandler = Rc<dyn Fn(&Launch, &mut Window, &mut App)>;
type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

#[derive(IntoElement)]
pub struct AgentLauncher {
    id: SharedString,
    title: SharedString,
    subtitle: Option<SharedString>,
    modes: Vec<LaunchMode>,
    /// One line under the modes saying what the chosen one will do.
    mode_hint: Option<SharedString>,
    on_mode: Option<CommandHandler>,
    style: LauncherStyle,
    options: Vec<LaunchOption>,
    /// Command drawn as the default, so a quick start is not a guess.
    preferred: Option<SharedString>,
    sessions: Vec<LauncherSession>,
    /// Shown in place of the buttons while a start is in flight.
    busy: Option<SharedString>,
    /// Why nothing can be started right now, shown in place of the buttons.
    disabled: Option<SharedString>,
    /// What the start needs beside the agent — a request to type, say — drawn
    /// under the header.
    body: Option<AnyElement>,
    /// Whether the start buttons stay once a session exists. Off by default:
    /// for most things a second session duplicates the first.
    launch_alongside_sessions: bool,
    /// The brief every agent option starts with; `None` for a launcher that
    /// briefs nothing.
    brief: Option<LaunchBrief>,
    on_launch: Option<LaunchHandler>,
    on_configure: Option<(SharedString, ClickHandler)>,
    on_open: Option<CommandHandler>,
}

impl AgentLauncher {
    pub fn new(id: impl Into<SharedString>, title: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            subtitle: None,
            modes: Vec::new(),
            mode_hint: None,
            on_mode: None,
            style: LauncherStyle::default(),
            options: Vec::new(),
            preferred: None,
            sessions: Vec::new(),
            busy: None,
            disabled: None,
            body: None,
            launch_alongside_sessions: false,
            brief: None,
            on_launch: None,
            on_configure: None,
            on_open: None,
        }
    }

    /// Offer a choice of what launching will do. Empty hides the row.
    pub fn modes(mut self, modes: Vec<LaunchMode>) -> Self {
        self.modes = modes;
        self
    }

    /// One line under the modes, saying what the chosen one will do.
    pub fn mode_hint(mut self, hint: impl Into<SharedString>) -> Self {
        self.mode_hint = Some(hint.into());
        self
    }

    /// Called with a mode's id when it is picked.
    pub fn on_mode(
        mut self,
        handler: impl Fn(&SharedString, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_mode = Some(Rc::new(handler));
        self
    }

    pub fn subtitle(mut self, subtitle: impl Into<SharedString>) -> Self {
        self.subtitle = Some(subtitle.into());
        self
    }

    pub fn style(mut self, style: LauncherStyle) -> Self {
        self.style = style;
        self
    }

    pub fn options(mut self, options: Vec<LaunchOption>) -> Self {
        self.options = options;
        self
    }

    pub fn preferred(mut self, command: Option<impl Into<SharedString>>) -> Self {
        self.preferred = command.map(Into::into);
        self
    }

    pub fn sessions(mut self, sessions: Vec<LauncherSession>) -> Self {
        self.sessions = sessions;
        self
    }

    pub fn busy(mut self, label: Option<impl Into<SharedString>>) -> Self {
        self.busy = label.map(Into::into);
        self
    }

    /// Refuse to start, saying why where the buttons would be. The sessions
    /// stay listed and openable.
    pub fn disabled(mut self, reason: Option<impl Into<SharedString>>) -> Self {
        self.disabled = reason.map(Into::into);
        self
    }

    /// Draw `body` between the header and the sessions.
    pub fn body(mut self, body: impl IntoElement) -> Self {
        self.body = Some(body.into_any_element());
        self
    }

    pub fn launch_alongside_sessions(mut self) -> Self {
        self.launch_alongside_sessions = true;
        self
    }

    /// Name the brief the agent options start with, and the model each runs.
    /// `None` — still loading, or nothing briefed — shows no line.
    pub fn brief(mut self, brief: Option<LaunchBrief>) -> Self {
        self.brief = brief;
        self
    }

    /// Called with the chosen option's command and the model picked for it.
    pub fn on_launch(mut self, handler: impl Fn(&Launch, &mut Window, &mut App) + 'static) -> Self {
        self.on_launch = Some(Rc::new(handler));
        self
    }

    /// Offer a "configure first" button, described by `tooltip`.
    pub fn on_configure(
        mut self,
        tooltip: impl Into<SharedString>,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_configure = Some((tooltip.into(), Rc::new(handler)));
        self
    }

    /// Called with a session's id when it is clicked.
    pub fn on_open(
        mut self,
        handler: impl Fn(&SharedString, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_open = Some(Rc::new(handler));
        self
    }

    /// A start in flight always says so, sessions or not: the one it is
    /// starting is not among them yet.
    fn shows_buttons(&self) -> bool {
        self.busy.is_some() || self.sessions.is_empty() || self.launch_alongside_sessions
    }

    /// The round "configure first" button, beside the title.
    fn render_configure(&self, cx: &App) -> Option<AnyElement> {
        let (tooltip, handler) = self.on_configure.clone()?;
        if self.busy.is_some() || self.disabled.is_some() {
            return None;
        }
        let t = theme(cx);
        Some(
            div()
                .id(SharedString::from(format!("{}-configure", self.id)))
                .flex_shrink_0()
                .cursor_pointer()
                .size(px(BUTTON_SIZE))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .border_1()
                .border_color(rgb(t.border))
                .hover(|s| s.bg(rgb(t.bg_hover)))
                .child(
                    svg()
                        .path("icons/settings.svg")
                        .size(px(ICON_SIZE))
                        .text_color(rgb(t.text_secondary)),
                )
                .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
                .on_click(move |event, window, cx| {
                    cx.stop_propagation();
                    handler(event, window, cx);
                })
                .into_any_element(),
        )
    }

    /// The brief every agent starts with, as one quiet line under the title.
    /// Where the template comes from is its tooltip.
    fn render_brief(&self, cx: &App) -> Option<AnyElement> {
        let brief = self.brief.clone()?;
        let t = theme(cx);
        let source = brief.source.clone();
        Some(
            h_flex()
                .id(SharedString::from(format!("{}-brief", self.id)))
                .min_w_0()
                .max_w_full()
                .items_center()
                .gap(px(4.0))
                .text_size(ui_text_ms(cx))
                .text_color(rgb(t.text_muted))
                .child(
                    svg()
                        .path("icons/file-text.svg")
                        .flex_shrink_0()
                        .size(px(11.0))
                        .text_color(rgb(t.text_muted)),
                )
                .child(div().min_w_0().truncate().child(brief.name.clone()))
                .tooltip(move |window, cx| Tooltip::new(source.clone()).build(window, cx))
                .into_any_element(),
        )
    }

    /// The option the pill would start: the one chosen in its menu, else the
    /// default, else the first.
    fn chosen_option(&self, state: &LauncherState) -> Option<&LaunchOption> {
        let by = |command: &SharedString| self.options.iter().find(|o| &o.command == command);
        state
            .chosen
            .as_ref()
            .and_then(by)
            .or_else(|| self.preferred.as_ref().and_then(by))
            .or_else(|| self.options.first())
    }

    /// The one control that starts: `[agent ▾ │ model ▾ │ ▶]`. The left side
    /// picks what starts — an agent, or an option that starts none — the right
    /// side the model it runs on for this launch, and the button starts it. In
    /// its place, why nothing can start, or that one is.
    fn render_agents(&self, state: &Entity<LauncherState>, cx: &App) -> Option<AnyElement> {
        let t = theme(cx);
        if let Some(label) = self.busy.clone() {
            return Some(
                h_flex()
                    .h(px(PILL_HEIGHT))
                    .items_center()
                    .gap(px(6.0))
                    .child(div().size(px(6.0)).rounded_full().bg(rgb(t.warning)))
                    .child(
                        div()
                            .text_size(ui_text_md(cx))
                            .text_color(rgb(t.text_secondary))
                            .child(label),
                    )
                    .into_any_element(),
            );
        }
        if let Some(reason) = self.disabled.clone() {
            return Some(
                div()
                    .min_w_0()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .child(reason)
                    .into_any_element(),
            );
        }

        let s = state.read(cx);
        let option = self.chosen_option(s)?.clone();
        let accent = option.accent;
        let picked = s.picks.get(&option.command).cloned();
        let model = model_label(self.brief.as_ref(), &option.command, picked.as_deref());
        let menu = s.menu;
        let divider = || div().w(px(1.0)).h(px(14.0)).bg(with_alpha(accent, 0.25));
        let toggle = |which: Menu| {
            let state = state.clone();
            move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                cx.stop_propagation();
                state.update(cx, |state, cx| {
                    state.menu = (state.menu != Some(which)).then_some(which);
                    cx.notify();
                });
            }
        };

        let mut agent = h_flex()
            .id(SharedString::from(format!("{}-agent", self.id)))
            .relative()
            // Inset and round, like the start button: the hover then follows
            // the pill's own shape instead of squaring off its end.
            .h(px(SEGMENT_HEIGHT))
            .ml(px(2.0))
            .rounded_full()
            .items_center()
            .gap(px(6.0))
            .pl(px(8.0))
            .pr(px(7.0))
            .cursor_pointer()
            .hover(move |s| s.bg(with_alpha(accent, 0.18)))
            .when(!option.command.is_empty(), |d| {
                d.child(
                    svg()
                        .path(option.icon.clone())
                        .flex_shrink_0()
                        .size(px(13.0))
                        .text_color(rgb(accent)),
                )
            })
            .child(
                div()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_primary))
                    .child(option.label.clone()),
            )
            .child(chevron(&t))
            .tooltip(|window, cx| Tooltip::new("Choose what starts").build(window, cx))
            .on_click(toggle(Menu::Agent));
        if menu == Some(Menu::Agent) {
            agent = agent.child(self.render_agent_menu(state, &option.command, cx));
        }

        let mut pill = h_flex()
            .flex_shrink_0()
            .h(px(PILL_HEIGHT))
            .items_center()
            .rounded(px(PILL_HEIGHT / 2.0))
            .border_1()
            .border_color(with_alpha(accent, 0.35))
            .bg(with_alpha(accent, 0.07))
            .child(agent);

        if let Some(model) = model {
            let choices = agent_model::choices(&option.command);
            let mut part = h_flex()
                .id(SharedString::from(format!("{}-model", self.id)))
                .relative()
                .h(px(SEGMENT_HEIGHT))
                .mx(px(2.0))
                .rounded_full()
                .items_center()
                .gap(px(4.0))
                .px(px(7.0))
                .text_size(ui_text_ms(cx))
                .text_color(rgb(if picked.is_some() {
                    t.text_primary
                } else {
                    t.text_secondary
                }))
                .child(model);
            if !choices.is_empty() {
                part = part
                    .cursor_pointer()
                    .hover(move |s| s.bg(with_alpha(accent, 0.18)))
                    .child(chevron(&t))
                    .tooltip(|window, cx| Tooltip::new("Model for this launch").build(window, cx))
                    .on_click(toggle(Menu::Model));
                if menu == Some(Menu::Model) {
                    part = part.child(self.render_model_menu(
                        state,
                        &option.command,
                        picked.as_deref(),
                        choices,
                        cx,
                    ));
                }
            }
            pill = pill.child(divider()).child(part);
        }

        let handler = self.on_launch.clone();
        let command = option.command.clone();
        let start_tooltip = option.tooltip();
        let start = div()
            .id(SharedString::from(format!("{}-start", self.id)))
            .flex_shrink_0()
            .size(px(SEGMENT_HEIGHT))
            .mx(px(2.0))
            .rounded_full()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .bg(with_alpha(accent, 0.85))
            .hover(move |s| s.bg(rgb(accent)))
            .child(
                svg()
                    .path("icons/play.svg")
                    .size(px(10.0))
                    .text_color(rgb(t.bg_primary)),
            )
            .tooltip(move |window, cx| Tooltip::new(start_tooltip.clone()).build(window, cx))
            .on_click(move |_, window, cx| {
                cx.stop_propagation();
                if let Some(handler) = handler.as_ref() {
                    let launch = Launch {
                        command: command.clone(),
                        model: picked.clone(),
                    };
                    handler(&launch, window, cx);
                }
            });
        Some(pill.child(start).into_any_element())
    }

    /// Every option, agents first, then the ones that start none.
    fn render_agent_menu(
        &self,
        state: &Entity<LauncherState>,
        current: &SharedString,
        cx: &App,
    ) -> AnyElement {
        let t = theme(cx);
        let mut menu = menu_panel(format!("{}-agent-menu", self.id), state, &t);
        let (agents, plain): (Vec<&LaunchOption>, Vec<&LaunchOption>) =
            self.options.iter().partition(|o| !o.command.is_empty());
        let row = |menu: Stateful<Div>, option: &LaunchOption, index: usize| {
            let choose = state.clone();
            let command = option.command.clone();
            let preferred = self.preferred.as_ref() == Some(&option.command);
            menu.child(
                menu_row(
                    format!("{}-agent-{index}", self.id),
                    option.label.clone(),
                    &option.command == current,
                    &t,
                    cx,
                )
                .when(!option.command.is_empty(), |d| {
                    d.child(
                        svg()
                            .path(option.icon.clone())
                            .size(px(12.0))
                            .text_color(rgb(option.accent)),
                    )
                })
                .when(preferred, |d| {
                    d.child(
                        div()
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(t.text_muted))
                            .child("default"),
                    )
                })
                .on_click(move |_, _window, cx| {
                    cx.stop_propagation();
                    choose.update(cx, |state, cx| {
                        state.chosen = Some(command.clone());
                        state.menu = None;
                        cx.notify();
                    });
                }),
            )
        };
        for (index, option) in agents.iter().enumerate() {
            menu = row(menu, option, index);
        }
        if !plain.is_empty() {
            menu = menu.child(div().my(px(4.0)).h(px(1.0)).bg(rgb(t.border)));
            for (index, option) in plain.iter().enumerate() {
                menu = row(menu, option, 100 + index);
            }
        }
        anchor_under(menu)
    }

    fn render_model_menu(
        &self,
        state: &Entity<LauncherState>,
        command: &SharedString,
        picked: Option<&str>,
        choices: &[&'static str],
        cx: &App,
    ) -> AnyElement {
        let t = theme(cx);
        let mut menu = menu_panel(format!("{}-model-menu", self.id), state, &t);
        // `""` is the CLI's default, last: a fallback rather than a model.
        let rows = choices
            .iter()
            .map(|m| (SharedString::from(*m), *m))
            .chain(std::iter::once((SharedString::from("CLI default"), "")));
        for (label, value) in rows {
            let pick = state.clone();
            let command = command.clone();
            menu = menu.child(
                menu_row(
                    format!("{}-model-{label}", self.id),
                    label,
                    picked == Some(value),
                    &t,
                    cx,
                )
                .on_click(move |_, _window, cx| {
                    cx.stop_propagation();
                    pick.update(cx, |state, cx| {
                        state.picks.insert(command.clone(), value.to_string());
                        state.menu = None;
                        cx.notify();
                    });
                }),
            );
        }
        anchor_under(menu)
    }

    fn render_session(&self, session: &LauncherSession, cx: &App) -> AnyElement {
        let t = theme(cx);
        let handler = self.on_open.clone();
        let id = session.id.clone();

        let mut meta = h_flex().min_w_0().items_center().gap(px(6.0));
        if let Some(agent) = session.agent.as_ref() {
            meta = meta.child(
                h_flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap(px(3.0))
                    .child(
                        svg()
                            .path(agent.icon.clone())
                            .size(px(10.0))
                            .text_color(rgb(agent.accent)),
                    )
                    .child(
                        div()
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(t.text_secondary))
                            .child(agent.label.clone()),
                    ),
            );
        }
        meta = meta.child(
            div()
                .flex_shrink_0()
                .text_size(ui_text_ms(cx))
                .text_color(rgb(session.activity_color))
                .child(session.activity.clone()),
        );
        if let Some(warning) = session.warning.clone() {
            meta = meta.child(
                div()
                    .flex_shrink_0()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .child(format!("· {warning}")),
            );
        }

        h_flex()
            .id(SharedString::from(format!(
                "{}-session-{}",
                self.id, session.id
            )))
            .w_full()
            .min_w_0()
            .items_center()
            .gap(px(8.0))
            .px(px(8.0))
            .py(px(6.0))
            .rounded(px(6.0))
            .bg(rgb(t.bg_primary))
            .when(handler.is_some(), |d| {
                d.cursor_pointer().hover(|s| s.bg(rgb(t.bg_hover)))
            })
            .child(
                div()
                    .flex_shrink_0()
                    .size(px(8.0))
                    .rounded_full()
                    .bg(rgb(session.activity_color)),
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
                            .text_size(ui_text_md(cx))
                            .text_color(rgb(t.text_primary))
                            .child(session.name.clone()),
                    )
                    .child(meta)
                    .children(session.status.clone().map(|status| {
                        div()
                            .w_full()
                            .min_w_0()
                            .truncate()
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(t.text_secondary))
                            .child(format!("\u{201c}{status}\u{201d}"))
                    })),
            )
            .when(handler.is_some(), |d| {
                d.child(
                    h_flex()
                        .flex_shrink_0()
                        .items_center()
                        .gap(px(2.0))
                        .text_size(ui_text_ms(cx))
                        .text_color(rgb(t.text_secondary))
                        .child("Open")
                        .child(
                            svg()
                                .path("icons/chevron-right.svg")
                                .size(px(10.0))
                                .text_color(rgb(t.text_secondary)),
                        ),
                )
            })
            .on_click(move |_, window, cx| {
                if let Some(handler) = handler.as_ref() {
                    cx.stop_propagation();
                    handler(&id, window, cx);
                }
            })
            .into_any_element()
    }
}

impl AgentLauncher {
    /// The mode row: what launching will do, and a line saying so.
    fn render_modes(&self, cx: &App) -> Option<AnyElement> {
        if self.modes.is_empty() {
            return None;
        }
        let t = theme(cx);
        let mut row = h_flex().gap(px(4.0)).flex_wrap();
        for mode in &self.modes {
            let handler = self.on_mode.clone();
            let id = mode.id.clone();
            let selected = mode.selected;
            row = row.child(
                div()
                    .id(SharedString::from(format!("{}-mode-{}", self.id, mode.id)))
                    .cursor_pointer()
                    .flex_shrink_0()
                    .px(px(8.0))
                    .py(px(2.0))
                    .rounded(px(4.0))
                    .text_size(ui_text_ms(cx))
                    .map(|el| {
                        if selected {
                            el.bg(with_alpha(t.border_active, 0.22))
                                .text_color(rgb(t.text_primary))
                        } else {
                            el.bg(rgb(t.bg_primary))
                                .text_color(rgb(t.text_muted))
                                .hover(|s| s.bg(rgb(t.bg_hover)).text_color(rgb(t.text_secondary)))
                        }
                    })
                    .child(mode.label.clone())
                    .on_click(move |_, window, cx| {
                        if let Some(handler) = handler.as_ref() {
                            cx.stop_propagation();
                            handler(&id, window, cx);
                        }
                    }),
            );
        }
        Some(
            v_flex()
                .w_full()
                .gap(px(3.0))
                .child(row)
                .children(self.mode_hint.clone().map(|hint| {
                    div()
                        .w_full()
                        .min_w_0()
                        .text_size(ui_text_ms(cx))
                        .text_color(rgb(t.text_muted))
                        .child(hint)
                }))
                .into_any_element(),
        )
    }
}

fn chevron(t: &crate::theme::ThemeColors) -> Svg {
    svg()
        .path("icons/chevron-down.svg")
        .flex_shrink_0()
        .size(px(10.0))
        .text_color(rgb(t.text_muted))
}

/// A menu's panel; pressing anywhere outside it closes it.
fn menu_panel(
    id: String,
    state: &Entity<LauncherState>,
    t: &crate::theme::ThemeColors,
) -> Stateful<Div> {
    let close = state.clone();
    v_flex()
        .id(SharedString::from(id))
        .occlude()
        .min_w(px(170.0))
        .py(px(4.0))
        .bg(rgb(t.bg_primary))
        .border_1()
        .border_color(rgb(t.border))
        .rounded(px(6.0))
        .shadow_lg()
        .on_mouse_down_out(move |_, _window, cx| {
            close.update(cx, |state, cx| {
                state.menu = None;
                cx.notify();
            });
        })
}

/// One menu row: a check for the current one, then its label; callers add
/// what trails it.
fn menu_row(
    id: String,
    label: impl Into<SharedString>,
    current: bool,
    t: &crate::theme::ThemeColors,
    cx: &App,
) -> Stateful<Div> {
    h_flex()
        .id(SharedString::from(id))
        .px(px(8.0))
        .py(px(4.0))
        .gap(px(6.0))
        .items_center()
        .cursor_pointer()
        .text_size(ui_text_ms(cx))
        .text_color(rgb(t.text_primary))
        .hover(|s| s.bg(rgb(t.bg_hover)))
        .child(div().w(px(12.0)).flex_shrink_0().when(current, |d| {
            d.child(
                svg()
                    .path("icons/check.svg")
                    .size(px(11.0))
                    .text_color(rgb(t.border_active)),
            )
        }))
        .child(div().flex_1().child(label.into()))
}

/// Pin `menu` under the corner of the part it opens from, above everything.
fn anchor_under(menu: Stateful<Div>) -> AnyElement {
    div()
        .absolute()
        .top_0()
        .left_0()
        .child(
            deferred(
                anchored()
                    .snap_to_window()
                    .child(div().mt(px(SEGMENT_HEIGHT + 6.0)).child(menu)),
            )
            .with_priority(1),
        )
        .into_any_element()
}

/// The round configure button.
const BUTTON_SIZE: f32 = 28.0;
const ICON_SIZE: f32 = 14.0;
/// The start pill.
const PILL_HEIGHT: f32 = 28.0;
/// Each part inside the pill — agent, model, start — as a pill of its own.
const SEGMENT_HEIGHT: f32 = PILL_HEIGHT - 6.0;

impl RenderOnce for AgentLauncher {
    fn render(mut self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = theme(cx);
        let body = self.body.take();
        let state = window.use_keyed_state(
            SharedString::from(format!("{}-state", self.id)),
            cx,
            |_, _| LauncherState::default(),
        );
        let starts = self.shows_buttons();
        let brief = starts.then(|| self.render_brief(cx)).flatten();
        let configure = starts.then(|| self.render_configure(cx)).flatten();
        let agents = starts.then(|| self.render_agents(&state, cx)).flatten();

        let header = h_flex()
            .w_full()
            .min_w_0()
            .items_center()
            .gap(px(10.0))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(2.0))
                    .child(
                        div()
                            .w_full()
                            .min_w_0()
                            .truncate()
                            .text_size(ui_text_md(cx))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(t.text_primary))
                            .child(self.title.clone()),
                    )
                    .children(self.subtitle.clone().map(|subtitle| {
                        div()
                            .w_full()
                            .min_w_0()
                            .truncate()
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(t.text_muted))
                            .child(subtitle)
                    }))
                    .children(brief),
            )
            .when(configure.is_some() || agents.is_some(), |d| {
                d.child(
                    h_flex()
                        .flex_shrink_0()
                        .items_center()
                        .gap(px(6.0))
                        .children(configure)
                        .children(agents),
                )
            });

        let modes = self.render_modes(cx);

        let sessions: Vec<AnyElement> = self
            .sessions
            .iter()
            .map(|session| self.render_session(session, cx))
            .collect();

        v_flex()
            .id(self.id.clone())
            .w_full()
            .min_w_0()
            .gap(px(8.0))
            .when(self.style == LauncherStyle::Card, |d| {
                d.px(px(12.0))
                    .py(px(10.0))
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(rgb(t.border))
                    .bg(rgb(t.bg_secondary))
            })
            .child(header)
            .children(modes)
            .children(body)
            .children(sessions)
    }
}

#[cfg(test)]
mod tests {
    use super::{AgentLauncher, LaunchBrief, LaunchOption, LauncherState, model_label};
    use okena_core::agent_model::AgentModels;

    fn option(command: &str) -> LaunchOption {
        LaunchOption {
            command: command.to_string().into(),
            label: command.to_string().into(),
            icon: Default::default(),
            accent: 0,
        }
    }

    #[test]
    fn the_pill_starts_the_chosen_option_else_the_default_else_the_first() {
        let launcher = AgentLauncher::new("l", "Start")
            .options(vec![option("claude"), option("codex"), option("")])
            .preferred(Some("codex"));
        let chosen = |c: Option<&str>| {
            let state = LauncherState {
                chosen: c.map(|c| c.to_string().into()),
                ..Default::default()
            };
            launcher
                .chosen_option(&state)
                .map(|o| o.command.to_string())
        };
        assert_eq!(chosen(None).as_deref(), Some("codex"));
        assert_eq!(chosen(Some("claude")).as_deref(), Some("claude"));
        // "Plain shell" can be chosen too; it starts no agent.
        assert_eq!(chosen(Some("")).as_deref(), Some(""));
        // A choice that is no longer offered falls back to the default.
        assert_eq!(chosen(Some("copilot")).as_deref(), Some("codex"));
        let no_default = AgentLauncher::new("l", "Start").options(vec![option("claude")]);
        assert_eq!(
            no_default
                .chosen_option(&LauncherState::default())
                .map(|o| o.command.to_string())
                .as_deref(),
            Some("claude")
        );
    }

    fn brief(model: Option<&str>) -> LaunchBrief {
        LaunchBrief {
            name: "task-start".into(),
            source: "okena's built-in `task-start` template".into(),
            models: AgentModels::new(model.map(str::to_string), Default::default()),
        }
    }

    #[test]
    fn a_pill_names_the_model_its_agent_will_run() {
        let opus = brief(Some("opus"));
        assert_eq!(
            model_label(Some(&opus), "claude", None).as_deref(),
            Some("opus")
        );
        // A pick wins; "CLI default" reads as the default.
        assert_eq!(
            model_label(Some(&opus), "claude", Some("haiku")).as_deref(),
            Some("haiku")
        );
        assert_eq!(
            model_label(Some(&opus), "claude", Some("")).as_deref(),
            Some("default")
        );
        // A Claude alias is not Codex's: it runs its own default.
        assert_eq!(
            model_label(Some(&opus), "codex", None).as_deref(),
            Some("default")
        );
    }

    #[test]
    fn nothing_briefed_or_no_model_flag_means_no_model_part() {
        let opus = brief(Some("opus"));
        assert_eq!(model_label(None, "claude", None), None);
        // Plain shell, worktrees only, file it.
        assert_eq!(model_label(Some(&opus), "", None), None);
        // A command okena never hands a model.
        assert_eq!(model_label(Some(&opus), "aider", None), None);
    }
}
