//! Write okena extensions in Rust.
//!
//! An extension is a WASM component that okena's daemon runs. It returns a
//! declarative view that okena draws natively, offers actions and queries, and
//! reaches the machine only through host calls the user approved at install.
//!
//! ```ignore
//! use okena_extension_api::{self as okena, ui, Extension, Refresh};
//!
//! struct Hello;
//!
//! impl Extension for Hello {
//!     fn new() -> Self { Hello }
//!
//!     fn refresh(&mut self) -> okena::Result<Refresh> {
//!         let mut view = ui::View::new();
//!         let root = view.add(ui::text("Hello from an extension"));
//!         Ok(Refresh::new(view.finish(root)))
//!     }
//! }
//!
//! okena::register_extension!(Hello);
//! ```
//!
//! Build it with `cargo build --release --target wasm32-wasip2`; see
//! `docs/reference/extensions.md` in the okena repo.

pub mod agent;
pub mod host;
pub mod ui;

use std::collections::HashMap;

/// The generated bindings. Extensions normally use the helpers instead.
#[doc(hidden)]
pub mod wit {
    wit_bindgen::generate!({
        path: "wit",
        world: "extension",
        pub_export_macro: true,
        default_bindings_module: "okena_extension_api::wit",
    });
}

pub use serde_json;

pub use agent::{AgentLaunch, ContextRef};
pub use host::{Command, CommandOutput, DirEntry, Project};
pub use ui::Tone;
pub use wit::exports::okena::extension::guest::{AgentMode, Invoker};

pub type Result<T, E = String> = std::result::Result<T, E>;

/// An okena extension. One value lives for as long as the extension is
/// loaded, so it can keep state between calls.
pub trait Extension: 'static {
    fn new() -> Self
    where
        Self: Sized;

    /// The actions and queries it offers. Asked once, after loading.
    fn describe(&self) -> Info {
        Info::default()
    }

    /// Builds the view. Called on the manifest's interval, on demand, after
    /// a configuration change, and after an action that asks for it.
    fn refresh(&mut self) -> Result<Refresh>;

    fn run_action(&mut self, request: ActionRequest) -> Result<ActionOutcome> {
        Err(format!("unknown action `{}`", request.action))
    }

    /// Answers a query; `args` and the answer are JSON. Agents the extension
    /// started call queries through okena's MCP.
    fn query(&mut self, id: &str, args: serde_json::Value) -> Result<serde_json::Value> {
        let _ = args;
        Err(format!("unknown query `{id}`"))
    }
}

/// What [`Extension::describe`] returns.
#[derive(Default, Clone, Debug)]
pub struct Info {
    pub actions: Vec<Action>,
    pub queries: Vec<Query>,
}

impl Info {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn action(mut self, action: Action) -> Self {
        self.actions.push(action);
        self
    }

    pub fn query(mut self, query: Query) -> Self {
        self.queries.push(query);
        self
    }
}

pub use wit::exports::okena::extension::guest::InputKind;

/// A field of an action's form, or a query's parameter.
#[derive(Clone, Debug)]
pub struct Input {
    pub key: String,
    pub label: String,
    pub kind: InputKind,
    pub required: bool,
    pub default: Option<String>,
    pub placeholder: Option<String>,
    pub options: Vec<String>,
}

impl Input {
    fn new(key: &str, label: &str, kind: InputKind) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            kind,
            required: false,
            default: None,
            placeholder: None,
            options: Vec::new(),
        }
    }

    pub fn text(key: &str, label: &str) -> Self {
        Self::new(key, label, InputKind::Text)
    }

    pub fn multiline(key: &str, label: &str) -> Self {
        Self::new(key, label, InputKind::Multiline)
    }

    pub fn number(key: &str, label: &str) -> Self {
        Self::new(key, label, InputKind::Number)
    }

    pub fn toggle(key: &str, label: &str) -> Self {
        Self::new(key, label, InputKind::Toggle)
    }

    pub fn select<S: Into<String>>(
        key: &str,
        label: &str,
        options: impl IntoIterator<Item = S>,
    ) -> Self {
        let mut input = Self::new(key, label, InputKind::Select);
        input.options = options.into_iter().map(Into::into).collect();
        input
    }

    pub fn required(mut self) -> Self {
        self.required = true;
        self
    }

    pub fn default_value(mut self, value: impl Into<String>) -> Self {
        self.default = Some(value.into());
        self
    }

    pub fn placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = Some(placeholder.into());
        self
    }

    fn into_wit(self) -> wit::exports::okena::extension::guest::Input {
        wit::exports::okena::extension::guest::Input {
            key: self.key,
            label: self.label,
            kind: self.kind,
            required: self.required,
            default: self.default,
            placeholder: self.placeholder,
            options: self.options,
        }
    }
}

/// An action okena offers as a button: on each row, on the selected rows, or
/// on the whole view, wherever the view lists its id.
#[derive(Clone, Debug)]
pub struct Action {
    pub id: String,
    pub label: String,
    pub description: String,
    pub destructive: bool,
    pub inputs: Vec<Input>,
    pub agent: Option<AgentMode>,
    pub agent_callable: bool,
}

impl Action {
    pub fn new(id: &str, label: &str) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            description: String::new(),
            destructive: false,
            inputs: Vec::new(),
            agent: None,
            agent_callable: false,
        }
    }

    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }

    /// okena asks the user to confirm before it runs, whoever asks for it.
    pub fn destructive(mut self) -> Self {
        self.destructive = true;
        self
    }

    /// A field of the small form okena shows before running it.
    pub fn input(mut self, input: Input) -> Self {
        self.inputs.push(input);
        self
    }

    /// It launches an agent session: its outcome carries an [`AgentLaunch`].
    /// [`AgentMode::Start`] needs the `start_agents` permission.
    pub fn launches_agent(mut self, mode: AgentMode) -> Self {
        self.agent = Some(mode);
        self
    }

    /// Agents this extension started may call it through okena's MCP.
    pub fn agent_callable(mut self) -> Self {
        self.agent_callable = true;
        self
    }

    fn into_wit(self) -> wit::exports::okena::extension::guest::ActionDef {
        wit::exports::okena::extension::guest::ActionDef {
            id: self.id,
            label: self.label,
            description: self.description,
            destructive: self.destructive,
            inputs: self.inputs.into_iter().map(Input::into_wit).collect(),
            agent: self.agent,
            agent_callable: self.agent_callable,
        }
    }
}

/// A read-only question agents can ask through okena's MCP.
#[derive(Clone, Debug)]
pub struct Query {
    pub id: String,
    pub description: String,
    pub params: Vec<Input>,
}

impl Query {
    pub fn new(id: &str, description: &str) -> Self {
        Self {
            id: id.into(),
            description: description.into(),
            params: Vec::new(),
        }
    }

    pub fn param(mut self, param: Input) -> Self {
        self.params.push(param);
        self
    }

    fn into_wit(self) -> wit::exports::okena::extension::guest::QueryDef {
        wit::exports::okena::extension::guest::QueryDef {
            id: self.id,
            description: self.description,
            params: self.params.into_iter().map(Input::into_wit).collect(),
        }
    }
}

/// What [`Extension::refresh`] returns: the view, and the status bar label.
pub struct Refresh {
    pub view: ui::FinishedView,
    pub status: Option<ui::Status>,
}

impl Refresh {
    pub fn new(view: ui::FinishedView) -> Self {
        Self { view, status: None }
    }

    /// The status bar widget's label. Clicking it opens the view.
    pub fn status(mut self, label: impl Into<String>, tone: Option<Tone>) -> Self {
        self.status = Some(ui::Status {
            label: label.into(),
            tone,
            tooltip: None,
        });
        self
    }

    pub fn status_tooltip(mut self, tooltip: impl Into<String>) -> Self {
        if let Some(status) = &mut self.status {
            status.tooltip = Some(tooltip.into());
        }
        self
    }
}

/// An action to run, as okena asks for it.
#[derive(Clone, Debug)]
pub struct ActionRequest {
    pub action: String,
    /// The rows or items it runs on; empty for a view action.
    pub items: Vec<String>,
    /// The form's values, by input key.
    pub inputs: HashMap<String, String>,
    /// A user in okena, or an agent through okena's MCP.
    pub invoker: Invoker,
}

impl ActionRequest {
    pub fn input(&self, key: &str) -> Option<&str> {
        self.inputs.get(key).map(String::as_str)
    }

    /// The single item a row action runs on.
    pub fn item(&self) -> Result<&str> {
        match self.items.as_slice() {
            [item] => Ok(item),
            [] => Err(format!("`{}` needs a row", self.action)),
            _ => Err(format!("`{}` runs on one row at a time", self.action)),
        }
    }
}

/// What running an action reports back.
#[derive(Default, Clone, Debug)]
pub struct ActionOutcome {
    pub message: Option<String>,
    pub tone: Option<Tone>,
    pub agent: Option<AgentLaunch>,
    pub refresh: bool,
}

impl ActionOutcome {
    /// Reported to the user as a success.
    pub fn success(message: impl Into<String>) -> Self {
        Self {
            message: Some(message.into()),
            tone: Some(Tone::Success),
            ..Self::default()
        }
    }

    /// Hands okena an agent session to launch, the way the action's
    /// [`Action::launches_agent`] mode says.
    pub fn launch(agent: AgentLaunch) -> Self {
        Self {
            agent: Some(agent),
            ..Self::default()
        }
    }

    /// Refreshes the view afterwards.
    pub fn and_refresh(mut self) -> Self {
        self.refresh = true;
        self
    }
}

#[doc(hidden)]
pub mod __private {
    //! What [`register_extension!`] expands to calls these.
    use super::*;
    use std::cell::RefCell;
    use wit::exports::okena::extension::guest as g;

    pub use wit::export;

    thread_local! {
        static EXTENSION: RefCell<Option<Box<dyn Extension>>> = const { RefCell::new(None) };
    }

    fn with<R>(make: fn() -> Box<dyn Extension>, f: impl FnOnce(&mut dyn Extension) -> R) -> R {
        EXTENSION.with(|cell| {
            let mut slot = cell.borrow_mut();
            let extension = slot.get_or_insert_with(make);
            f(extension.as_mut())
        })
    }

    pub fn describe(make: fn() -> Box<dyn Extension>) -> g::Info {
        let info = with(make, |e| e.describe());
        g::Info {
            actions: info.actions.into_iter().map(Action::into_wit).collect(),
            queries: info.queries.into_iter().map(Query::into_wit).collect(),
        }
    }

    pub fn refresh(make: fn() -> Box<dyn Extension>) -> Result<g::RefreshOutput> {
        let refresh = with(make, |e| e.refresh())?;
        Ok(g::RefreshOutput {
            view: refresh.view.0,
            status: refresh.status,
        })
    }

    pub fn run_action(
        make: fn() -> Box<dyn Extension>,
        request: g::ActionRequest,
    ) -> Result<g::ActionOutcome> {
        let request = ActionRequest {
            action: request.action,
            items: request.items,
            inputs: request.inputs.into_iter().collect(),
            invoker: request.invoker,
        };
        let outcome = with(make, |e| e.run_action(request))?;
        Ok(g::ActionOutcome {
            message: outcome.message,
            tone: outcome.tone,
            agent: outcome.agent.map(AgentLaunch::into_wit),
            refresh: outcome.refresh,
        })
    }

    pub fn query(make: fn() -> Box<dyn Extension>, id: String, args: String) -> Result<String> {
        let args = if args.trim().is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_str(&args).map_err(|e| format!("query arguments are not JSON: {e}"))?
        };
        let answer = with(make, |e| e.query(&id, args))?;
        serde_json::to_string(&answer).map_err(|e| e.to_string())
    }
}

/// Makes `$t` the extension this component runs.
#[macro_export]
macro_rules! register_extension {
    ($t:ty) => {
        #[doc(hidden)]
        pub struct __OkenaExtension;

        #[doc(hidden)]
        fn __okena_make() -> ::std::boxed::Box<dyn $crate::Extension> {
            ::std::boxed::Box::new(<$t as $crate::Extension>::new())
        }

        impl $crate::wit::exports::okena::extension::guest::Guest for __OkenaExtension {
            fn describe() -> $crate::wit::exports::okena::extension::guest::Info {
                $crate::__private::describe(__okena_make)
            }

            fn refresh() -> ::std::result::Result<
                $crate::wit::exports::okena::extension::guest::RefreshOutput,
                ::std::string::String,
            > {
                $crate::__private::refresh(__okena_make)
            }

            fn run_action(
                request: $crate::wit::exports::okena::extension::guest::ActionRequest,
            ) -> ::std::result::Result<
                $crate::wit::exports::okena::extension::guest::ActionOutcome,
                ::std::string::String,
            > {
                $crate::__private::run_action(__okena_make, request)
            }

            fn query(
                id: ::std::string::String,
                args: ::std::string::String,
            ) -> ::std::result::Result<::std::string::String, ::std::string::String> {
                $crate::__private::query(__okena_make, id, args)
            }
        }

        $crate::__private::export!(__OkenaExtension with_types_in $crate::wit);
    };
}
