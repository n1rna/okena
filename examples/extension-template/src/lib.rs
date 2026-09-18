//! A starting point: the last commits of a configured checkout, as a table
//! with a detail pane, a row action and a status bar label.

use okena_extension_api::{
    self as okena, Action, ActionOutcome, ActionRequest, Command, Extension, Info, Refresh, Tone,
    host, ui,
};
use serde::Deserialize;

#[derive(Default, Deserialize)]
struct Config {
    #[serde(default)]
    folder: String,
}

struct MyExtension;

impl Extension for MyExtension {
    fn new() -> Self {
        MyExtension
    }

    fn describe(&self) -> Info {
        Info::new().action(Action::new("show", "Show").description("Show the commit's full message."))
    }

    fn refresh(&mut self) -> okena::Result<Refresh> {
        let config: Config = host::config()?;
        let log = Command::new("git")
            .args(["log", "-n", "50", "--format=%h%x1f%an%x1f%ar%x1f%s"])
            .current_dir(&config.folder)
            .run()?;

        let rows = log.lines().filter_map(|line| {
            let mut parts = line.split('\u{1f}');
            let (hash, author, when, subject) = (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
            Some(
                ui::Row::new(hash)
                    .cell(hash)
                    .cell(author)
                    .cell(when)
                    .cell(subject)
                    .detail(ui::Field::new("Commit", hash))
                    .detail(ui::Field::new("Author", author)),
            )
        });
        let table = ui::Table::new("commits")
            .column(ui::Column::text("hash", "Commit").unsortable())
            .column(ui::Column::text("author", "Author").groupable())
            .column(ui::Column::text("when", "When").unsortable())
            .column(ui::Column::text("subject", "Subject"))
            .rows(rows)
            .row_actions(["show"])
            .filter_placeholder("Filter commits");

        let mut view = ui::View::new();
        let title = view.add(ui::heading(&config.folder));
        let table = view.add(table.build());
        let root = view.stack([title, table]);
        Ok(Refresh::new(view.finish(root)).status("git log", Some(Tone::Info)))
    }

    fn run_action(&mut self, request: ActionRequest) -> okena::Result<ActionOutcome> {
        let config: Config = host::config()?;
        let hash = request.item()?;
        let message = Command::new("git")
            .args(["show", "-s", "--format=%B", hash])
            .current_dir(&config.folder)
            .run()?;
        Ok(ActionOutcome::success(message.trim().to_string()))
    }
}

okena::register_extension!(MyExtension);
