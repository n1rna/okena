//! cli-table: a table of jobs per tenant, computed by running `jq`.
//!
//! Shows what an extension can do with a CLI and a table: rows grouped by
//! tenant, row and bulk actions (one destructive), and two agent actions — one
//! that fills in okena's launcher, one that starts an agent at once.

use okena_extension_api::{
    self as okena, Action, ActionOutcome, ActionRequest, AgentLaunch, AgentMode, Command,
    Extension, Info, Input, Query, Refresh, Tone, host, serde_json, ui,
};
use serde::{Deserialize, Serialize};

const JOBS_KEY: &str = "jobs";
const SAMPLE: &str = include_str!("../sample-jobs.json");

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Job {
    id: String,
    tenant: String,
    status: String,
    attempts: u32,
    /// Unix seconds.
    started: i64,
    #[serde(default)]
    message: String,
}

/// A row as jq computes it: the job plus its age.
#[derive(Debug, Deserialize)]
struct Row {
    id: String,
    tenant: String,
    status: String,
    attempts: u32,
    age_minutes: f64,
    message: String,
}

#[derive(Default, Deserialize)]
struct Config {
    #[serde(default)]
    jobs_file: String,
    #[serde(default)]
    agent_root: String,
    #[serde(default)]
    project: String,
    #[serde(default = "default_stuck")]
    stuck_after_minutes: f64,
}

fn default_stuck() -> f64 {
    30.0
}

struct CliTable;

impl CliTable {
    fn config() -> Config {
        host::config().unwrap_or_default()
    }

    /// The jobs: from the configured file, else the sample kept in storage.
    fn jobs() -> okena::Result<Vec<Job>> {
        let config = Self::config();
        let raw = if config.jobs_file.trim().is_empty() {
            match host::storage::get(JOBS_KEY) {
                Some(raw) => raw,
                None => {
                    let jobs = Self::sample()?;
                    host::storage::set_json(JOBS_KEY, &jobs)?;
                    return Ok(jobs);
                }
            }
        } else {
            host::read_to_string(config.jobs_file.trim())?
        };
        serde_json::from_str(&raw).map_err(|e| format!("the jobs are not valid JSON: {e}"))
    }

    /// The sample, its start times counted back from now.
    fn sample() -> okena::Result<Vec<Job>> {
        #[derive(Deserialize)]
        struct SampleJob {
            id: String,
            tenant: String,
            status: String,
            attempts: u32,
            minutes_ago: i64,
            message: String,
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        let sample: Vec<SampleJob> =
            serde_json::from_str(SAMPLE).map_err(|e| format!("the sample is not valid JSON: {e}"))?;
        Ok(sample
            .into_iter()
            .map(|s| Job {
                id: s.id,
                tenant: s.tenant,
                status: s.status,
                attempts: s.attempts,
                started: now - s.minutes_ago * 60,
                message: s.message,
            })
            .collect())
    }

    fn save(jobs: &[Job]) -> okena::Result<()> {
        if !Self::config().jobs_file.trim().is_empty() {
            return Err("jobs from a file are read-only; clear Jobs file to use the sample".into());
        }
        host::storage::set_json(JOBS_KEY, &jobs)
    }

    /// Runs jq over the jobs: the CLI this extension depends on.
    fn rows(jobs: &[Job]) -> okena::Result<Vec<Row>> {
        let input = serde_json::to_string(jobs).map_err(|e| e.to_string())?;
        let out = Command::new("jq")
            .args([
                "-c",
                "now as $now | map({id, tenant, status, attempts, message: (.message // \"\"), age_minutes: ((($now - .started) / 60) | floor)})",
            ])
            .stdin(input)
            .run()?;
        serde_json::from_str(out.trim()).map_err(|e| format!("jq printed something unexpected: {e}"))
    }

    fn update(ids: &[String], f: impl Fn(&mut Job)) -> okena::Result<usize> {
        let mut jobs = Self::jobs()?;
        let mut changed = 0;
        for job in jobs.iter_mut().filter(|j| ids.contains(&j.id)) {
            f(job);
            changed += 1;
        }
        Self::save(&jobs)?;
        Ok(changed)
    }

    fn brief(job: &Job, age_minutes: i64) -> String {
        format!(
            "Job {id} for tenant {tenant} is {status} after {attempts} attempts and {age} minutes.\n\
             Its last message: {message}\n\n\
             Find out why it is stuck and whether it is safe to unblock. You can re-read the job \
             with okena's MCP tool okena_extension_query (extension cli-table, query get_job, \
             args {{\"id\": \"{id}\"}}) and unblock it with okena_extension_action (action unblock, \
             items [\"{id}\"]). Deleting it needs the user's confirmation in okena.",
            id = job.id,
            tenant = job.tenant,
            status = job.status,
            attempts = job.attempts,
            age = age_minutes,
            message = if job.message.is_empty() { "(none)" } else { &job.message },
        )
    }

    fn launch(request: &ActionRequest) -> okena::Result<ActionOutcome> {
        let id = request.item()?;
        let jobs = Self::jobs()?;
        let job = jobs.iter().find(|j| j.id == id).ok_or(format!("no job {id}"))?;
        let now = Self::rows(std::slice::from_ref(job))?
            .first()
            .map_or(0, |r| r.age_minutes as i64);
        let config = Self::config();
        let mut launch = AgentLaunch::new(Self::brief(job, now))
            .name(format!("Investigate {}", job.id))
            .item(&job.id, format!("{} ({})", job.id, job.tenant));
        if !config.agent_root.trim().is_empty() {
            launch = launch.root(config.agent_root.trim());
        }
        if !config.project.trim().is_empty()
            && let Some(project) = host::projects().into_iter().find(|p| p.name == config.project.trim())
        {
            launch = launch.project(project.id);
        }
        Ok(ActionOutcome::launch(launch))
    }
}

impl Extension for CliTable {
    fn new() -> Self {
        CliTable
    }

    fn describe(&self) -> Info {
        Info::new()
            .action(
                Action::new("unblock", "Unblock")
                    .description("Queue a blocked job again.")
                    .agent_callable(),
            )
            .action(
                Action::new("skip", "Skip")
                    .description("Mark the jobs skipped, with a reason.")
                    .input(Input::text("reason", "Reason").required().placeholder("Why skip them?")),
            )
            .action(
                Action::new("delete", "Delete")
                    .description("Remove the jobs for good.")
                    .destructive()
                    .agent_callable(),
            )
            .action(
                Action::new("investigate", "Investigate…")
                    .description("Open okena's agent launcher with a brief about this job.")
                    .launches_agent(AgentMode::Prefill),
            )
            .action(
                Action::new("investigate_now", "Investigate now")
                    .description("Start an agent on this job with the default agent.")
                    .launches_agent(AgentMode::Start),
            )
            .action(Action::new("reset", "Reset sample").description("Put the sample jobs back."))
            .query(
                Query::new("get_job", "One job, as JSON").param(Input::text("id", "Job id").required()),
            )
            .query(Query::new("list_jobs", "Every job, as JSON"))
    }

    fn refresh(&mut self) -> okena::Result<Refresh> {
        let config = Self::config();
        let rows = Self::rows(&Self::jobs()?)?;
        let stuck = |r: &Row| r.status == "blocked" || (r.status == "running" && r.age_minutes >= config.stuck_after_minutes);
        let stuck_count = rows.iter().filter(|r| stuck(r)).count();

        let table = ui::Table::new("jobs")
            .column(ui::Column::text("id", "Job"))
            .column(ui::Column::text("tenant", "Tenant").groupable())
            .column(ui::Column::badge("status", "Status").groupable())
            .column(ui::Column::number("attempts", "Attempts"))
            .column(ui::Column::number("age", "Age"))
            .column(ui::Column::text("message", "Message").unsortable())
            .rows(rows.iter().map(|r| {
                let tone = match r.status.as_str() {
                    "blocked" => Tone::Danger,
                    "running" if stuck(r) => Tone::Warning,
                    "running" => Tone::Info,
                    "done" => Tone::Success,
                    _ => Tone::Neutral,
                };
                ui::Row::new(&r.id)
                    .cell(r.id.as_str())
                    .cell(r.tenant.as_str())
                    .cell(ui::Cell::text(&r.status).tone(tone))
                    .cell(ui::Cell::number(r.attempts.into(), r.attempts.to_string()))
                    .cell(ui::Cell::number(r.age_minutes, format!("{} min", r.age_minutes)))
                    .cell(r.message.as_str())
                    .detail(ui::Field::new("Job", &r.id))
                    .detail(ui::Field::new("Tenant", &r.tenant))
                    .detail(ui::Field::new("Status", &r.status).tone(tone))
                    .detail(ui::Field::new("Attempts", r.attempts.to_string()))
                    .detail(ui::Field::new("Message", &r.message))
            }))
            .group_by("tenant")
            .sort_by("age", true)
            .row_actions(["unblock", "investigate", "investigate_now"])
            .bulk_actions(["skip", "delete"])
            .filter_placeholder("Filter jobs")
            .empty_text("No jobs");

        let mut view = ui::View::new();
        let stats = view.add(ui::stats([
            ui::Stat::new("Jobs", rows.len().to_string()),
            ui::Stat::new("Stuck", stuck_count.to_string())
                .tone(if stuck_count > 0 { Tone::Warning } else { Tone::Success })
                .hint(format!("blocked, or running over {} min", config.stuck_after_minutes)),
            ui::Stat::new("Tenants", {
                let mut tenants: Vec<&str> = rows.iter().map(|r| r.tenant.as_str()).collect();
                tenants.sort();
                tenants.dedup();
                tenants.len().to_string()
            }),
        ]));
        let table = view.add(table.build());
        let actions = view.add(ui::actions(["reset"]));
        let root = view.stack([stats, table, actions]);

        let refresh = Refresh::new(view.finish(root));
        Ok(if stuck_count > 0 {
            refresh
                .status(format!("{stuck_count} stuck"), Some(Tone::Warning))
                .status_tooltip("Jobs blocked or running too long")
        } else {
            refresh.status("jobs ok", Some(Tone::Success))
        })
    }

    fn run_action(&mut self, request: ActionRequest) -> okena::Result<ActionOutcome> {
        match request.action.as_str() {
            "unblock" => {
                let n = Self::update(&request.items, |job| {
                    job.status = "queued".into();
                    job.message = "unblocked".into();
                })?;
                Ok(ActionOutcome::success(format!("Unblocked {n} job(s)")).and_refresh())
            }
            "skip" => {
                let reason = request.input("reason").unwrap_or_default().to_string();
                let n = Self::update(&request.items, |job| {
                    job.status = "skipped".into();
                    job.message = format!("skipped: {reason}");
                })?;
                Ok(ActionOutcome::success(format!("Skipped {n} job(s)")).and_refresh())
            }
            "delete" => {
                let mut jobs = Self::jobs()?;
                let before = jobs.len();
                jobs.retain(|j| !request.items.contains(&j.id));
                Self::save(&jobs)?;
                Ok(ActionOutcome::success(format!("Deleted {} job(s)", before - jobs.len())).and_refresh())
            }
            "investigate" | "investigate_now" => Self::launch(&request),
            "reset" => {
                host::storage::delete(JOBS_KEY);
                Ok(ActionOutcome::success("Sample jobs restored").and_refresh())
            }
            other => Err(format!("unknown action {other}")),
        }
    }

    fn query(&mut self, id: &str, args: serde_json::Value) -> okena::Result<serde_json::Value> {
        let jobs = Self::jobs()?;
        match id {
            "get_job" => {
                let wanted = args.get("id").and_then(|v| v.as_str()).ok_or("give an `id`")?;
                let job = jobs.iter().find(|j| j.id == wanted).ok_or(format!("no job {wanted}"))?;
                serde_json::to_value(job).map_err(|e| e.to_string())
            }
            "list_jobs" => serde_json::to_value(&jobs).map_err(|e| e.to_string()),
            other => Err(format!("unknown query {other}")),
        }
    }
}

okena::register_extension!(CliTable);
