//! Each query exercises one host call or failure mode.

use okena_extension_api::{
    self as okena, Action, ActionOutcome, ActionRequest, AgentLaunch, AgentMode, Command,
    Extension, Info, Query, Refresh, host, serde_json::{Value, json}, ui,
};

struct Probe {
    refreshes: u32,
}

fn arg<'a>(args: &'a Value, key: &str) -> &'a str {
    args.get(key).and_then(Value::as_str).unwrap_or_default()
}

impl Extension for Probe {
    fn new() -> Self {
        Probe { refreshes: 0 }
    }

    fn describe(&self) -> Info {
        Info::new()
            .action(Action::new("launch", "Investigate").launches_agent(AgentMode::Start))
            .action(Action::new("wipe", "Wipe").destructive().agent_callable())
            .query(Query::new("echo", "Runs echo"))
    }

    fn refresh(&mut self) -> okena::Result<Refresh> {
        self.refreshes += 1;
        let out = Command::new("echo").arg("hello").run()?;
        let mut view = ui::View::new();
        let table = view.add(
            ui::Table::new("rows")
                .column(ui::Column::text("name", "Name").groupable())
                .row(ui::Row::new("a").cell(out.trim()))
                .row_actions(["launch"])
                .build(),
        );
        let count = view.add(ui::text(format!("refresh {}", self.refreshes)));
        let root = view.stack([table, count]);
        Ok(Refresh::new(view.finish(root)).status(format!("{} rows", 1), None))
    }

    fn run_action(&mut self, request: ActionRequest) -> okena::Result<ActionOutcome> {
        match request.action.as_str() {
            "launch" => {
                let item = request.item()?;
                Ok(ActionOutcome::launch(
                    AgentLaunch::new(format!("Look into {item}")).item(item, format!("Row {item}")),
                ))
            }
            "wipe" => Ok(ActionOutcome::success(format!("wiped {}", request.items.len()))),
            other => Err(format!("unknown action {other}")),
        }
    }

    fn query(&mut self, id: &str, args: Value) -> okena::Result<Value> {
        match id {
            "run" => {
                let program = arg(&args, "program");
                let out = Command::new(program).args(
                    args.get("args")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str),
                )
                .output()?;
                Ok(json!({ "code": out.exit_code, "stdout": out.stdout }))
            }
            "stdin" => Ok(json!(Command::new("cat").stdin("piped").run()?)),
            "read" => Ok(json!(host::read_to_string(arg(&args, "path"))?)),
            "list" => Ok(json!(host::read_dir(arg(&args, "path"))?
                .into_iter()
                .map(|e| e.name)
                .collect::<Vec<_>>())),
            "kv_set" => {
                host::storage::set(arg(&args, "key"), arg(&args, "value"))?;
                Ok(Value::Null)
            }
            "kv_get" => Ok(json!(host::storage::get(arg(&args, "key")))),
            "config" => Ok(host::config_json()),
            "projects" => Ok(json!(host::projects().len())),
            "log" => {
                host::info("probe says hi");
                Ok(Value::Null)
            }
            "panic" => panic!("probe panicked on purpose"),
            "spin" => {
                #[allow(clippy::empty_loop)]
                loop {}
            }
            "hog" => {
                let mut blocks = Vec::new();
                loop {
                    blocks.push(vec![1u8; 16 * 1024 * 1024]);
                    if blocks.len() > 1_000_000 {
                        return Ok(json!(blocks.len()));
                    }
                }
            }
            "count" => Ok(json!(self.refreshes)),
            other => Err(format!("unknown query {other}")),
        }
    }
}

okena::register_extension!(Probe);
