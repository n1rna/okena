//! The services whose status the extension can show, and how each one's
//! status page is read.
//!
//! Most publish an Atlassian Statuspage (`/api/v2/summary.json`); GitLab's is
//! on status.io. Both are read into the same [`ServiceStatus`], so the bar and
//! the popover never care which kind a page is.

use jiff::Timestamp;

/// One service the bar can show.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ServiceId {
    Claude,
    Codex,
    GitHub,
    GitLab,
}

/// Where a service's status comes from.
#[derive(Clone, Copy, Debug)]
pub enum Source {
    /// An Atlassian Statuspage. `component` narrows it to one component —
    /// a page covering many products (Anthropic's, OpenAI's) is only about
    /// this one when the component says so. `None` reads the whole page.
    Statuspage {
        base: &'static str,
        component: Option<ComponentMatch>,
    },
    /// A status.io page, by its id.
    StatusIo { page_id: &'static str },
}

/// How a Statuspage component is recognised.
#[derive(Clone, Copy, Debug)]
pub enum ComponentMatch {
    Exact(&'static str),
    /// The name or anything starting with it and a space: OpenAI renamed
    /// "Codex" to "Codex API", and a later "Codex Cloud" should still match.
    Prefix(&'static str),
}

impl ComponentMatch {
    fn matches(self, name: &str) -> bool {
        match self {
            ComponentMatch::Exact(want) => name == want,
            ComponentMatch::Prefix(want) => {
                name == want || name.strip_prefix(want).is_some_and(|rest| rest.starts_with(' '))
            }
        }
    }
}

impl ServiceId {
    /// Every service, in the order the settings list and the bar show them.
    pub const ALL: [ServiceId; 4] = [
        ServiceId::Claude,
        ServiceId::Codex,
        ServiceId::GitHub,
        ServiceId::GitLab,
    ];

    /// Stable id, used in settings.
    pub const fn slug(self) -> &'static str {
        match self {
            ServiceId::Claude => "claude",
            ServiceId::Codex => "codex",
            ServiceId::GitHub => "github",
            ServiceId::GitLab => "gitlab",
        }
    }

    pub fn from_slug(slug: &str) -> Option<ServiceId> {
        ServiceId::ALL.into_iter().find(|s| s.slug() == slug)
    }

    /// How its requests are named in okena's HTTP log.
    pub const fn http_label(self) -> &'static str {
        match self {
            ServiceId::Claude => "status.claude",
            ServiceId::Codex => "status.codex",
            ServiceId::GitHub => "status.github",
            ServiceId::GitLab => "status.gitlab",
        }
    }

    /// The name on the bar.
    pub const fn label(self) -> &'static str {
        match self {
            ServiceId::Claude => "Claude",
            ServiceId::Codex => "Codex",
            ServiceId::GitHub => "GitHub",
            ServiceId::GitLab => "GitLab",
        }
    }

    /// What the settings row says it covers.
    pub const fn description(self) -> &'static str {
        match self {
            ServiceId::Claude => "Claude Code, from status.claude.com",
            ServiceId::Codex => "Codex, from status.openai.com",
            ServiceId::GitHub => "All of GitHub, from githubstatus.com",
            ServiceId::GitLab => "GitLab.com, from status.gitlab.com",
        }
    }

    pub const fn icon(self) -> &'static str {
        match self {
            ServiceId::Claude => "icons/agent-claude.svg",
            ServiceId::Codex => "icons/agent-codex.svg",
            ServiceId::GitHub => "icons/git-pull-request.svg",
            ServiceId::GitLab => "icons/git-branch.svg",
        }
    }

    /// The page a click opens.
    pub const fn page_url(self) -> &'static str {
        match self {
            ServiceId::Claude => "https://status.claude.com",
            ServiceId::Codex => "https://status.openai.com",
            ServiceId::GitHub => "https://www.githubstatus.com",
            ServiceId::GitLab => "https://status.gitlab.com",
        }
    }

    pub const fn source(self) -> Source {
        match self {
            ServiceId::Claude => Source::Statuspage {
                base: "https://status.claude.com",
                component: Some(ComponentMatch::Exact("Claude Code")),
            },
            ServiceId::Codex => Source::Statuspage {
                base: "https://status.openai.com",
                component: Some(ComponentMatch::Prefix("Codex")),
            },
            ServiceId::GitHub => Source::Statuspage {
                base: "https://www.githubstatus.com",
                component: None,
            },
            ServiceId::GitLab => Source::StatusIo {
                page_id: "5b36dc6502d06804c08349f7",
            },
        }
    }

    /// The URL polled for this service's status.
    pub fn api_url(self) -> String {
        match self.source() {
            Source::Statuspage { base, .. } => format!("{base}/api/v2/summary.json"),
            Source::StatusIo { page_id } => format!("https://api.status.io/1.0/status/{page_id}"),
        }
    }
}

/// How a service is doing, in the words the bar uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Health {
    Operational,
    Degraded,
    PartialOutage,
    MajorOutage,
    Maintenance,
    Unknown,
}

impl Health {
    pub const fn label(self) -> &'static str {
        match self {
            Health::Operational => "OK",
            Health::Degraded => "Degraded",
            Health::PartialOutage => "Partial outage",
            Health::MajorOutage => "Major outage",
            Health::Maintenance => "Maintenance",
            Health::Unknown => "Unknown",
        }
    }

    /// A Statuspage component's `status`.
    fn from_component(status: &str) -> Health {
        match status {
            "operational" => Health::Operational,
            "degraded_performance" => Health::Degraded,
            "partial_outage" => Health::PartialOutage,
            "major_outage" => Health::MajorOutage,
            "under_maintenance" => Health::Maintenance,
            _ => Health::Unknown,
        }
    }

    /// A Statuspage page's overall `status.indicator`.
    fn from_indicator(indicator: &str) -> Health {
        match indicator {
            "none" => Health::Operational,
            "minor" => Health::Degraded,
            "major" => Health::PartialOutage,
            "critical" => Health::MajorOutage,
            "maintenance" => Health::Maintenance,
            _ => Health::Unknown,
        }
    }

    /// A status.io `status_code`.
    fn from_status_io(code: u64) -> Health {
        match code {
            100 => Health::Operational,
            200 => Health::Maintenance,
            300 => Health::Degraded,
            400 => Health::PartialOutage,
            500 | 600 => Health::MajorOutage,
            _ => Health::Unknown,
        }
    }
}

/// One update posted to an incident.
#[derive(Clone, Debug, PartialEq)]
pub struct IncidentUpdate {
    pub status: String,
    pub body: String,
    /// When it was posted, already worded for display.
    pub created_at: String,
}

/// An unresolved incident on the service.
#[derive(Clone, Debug, PartialEq)]
pub struct Incident {
    pub name: String,
    /// "minor", "major", "critical"… as the page words it.
    pub impact: String,
    pub updates: Vec<IncidentUpdate>,
}

/// What a service's page said, last time it was read.
#[derive(Clone, Debug, PartialEq)]
pub struct ServiceStatus {
    pub health: Health,
    pub incidents: Vec<Incident>,
}

/// Read a service's status out of its page's response. `None` when the
/// response is not what the page publishes — say, the component was renamed.
pub fn parse(service: ServiceId, resp: &serde_json::Value) -> Option<ServiceStatus> {
    match service.source() {
        Source::Statuspage { component, .. } => parse_statuspage(resp, component),
        Source::StatusIo { .. } => parse_status_io(resp),
    }
}

fn parse_statuspage(resp: &serde_json::Value, component: Option<ComponentMatch>) -> Option<ServiceStatus> {
    let (health, component_id) = match component {
        Some(want) => {
            let found = resp["components"]
                .as_array()?
                .iter()
                .find(|c| c["name"].as_str().is_some_and(|name| want.matches(name)))?;
            (
                Health::from_component(found["status"].as_str()?),
                found["id"].as_str().map(str::to_string),
            )
        }
        None => (
            Health::from_indicator(resp["status"]["indicator"].as_str().unwrap_or("none")),
            None,
        ),
    };
    // The summary only ever lists unresolved incidents. On a page about many
    // products, only the ones touching this component count.
    let incidents = resp["incidents"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|incident| match &component_id {
            Some(id) => incident["components"]
                .as_array()
                .is_some_and(|comps| comps.iter().any(|c| c["id"].as_str() == Some(id.as_str()))),
            None => true,
        })
        .map(|incident| Incident {
            name: incident["name"].as_str().unwrap_or("Unknown").to_string(),
            impact: incident["impact"].as_str().unwrap_or("none").to_string(),
            updates: incident["incident_updates"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|u| IncidentUpdate {
                    status: u["status"].as_str().unwrap_or("").to_string(),
                    body: u["body"].as_str().unwrap_or("").to_string(),
                    created_at: format_timestamp(u["created_at"].as_str().unwrap_or("")),
                })
                .collect(),
        })
        .collect();
    Some(ServiceStatus { health, incidents })
}

fn parse_status_io(resp: &serde_json::Value) -> Option<ServiceStatus> {
    let result = &resp["result"];
    let health = Health::from_status_io(result["status_overall"]["status_code"].as_u64()?);
    let incidents = result["incidents"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|incident| Incident {
            name: incident["name"].as_str().unwrap_or("Unknown").to_string(),
            impact: status_io_impact(
                incident["current_status"]
                    .as_u64()
                    .or_else(|| incident["status_code"].as_u64()),
            ),
            updates: incident["messages"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|m| IncidentUpdate {
                    status: m["state"]
                        .as_str()
                        .map(str::to_string)
                        .or_else(|| m["status"].as_u64().map(|c| Health::from_status_io(c).label().to_string()))
                        .unwrap_or_default(),
                    body: m["details"].as_str().unwrap_or("").to_string(),
                    created_at: format_timestamp(m["datetime"].as_str().unwrap_or("")),
                })
                .collect(),
        })
        .collect();
    Some(ServiceStatus { health, incidents })
}

/// A status.io incident's code, in Statuspage's impact words so the popover
/// colours both alike.
fn status_io_impact(code: Option<u64>) -> String {
    match code {
        Some(500) | Some(600) => "critical",
        Some(400) => "major",
        _ => "minor",
    }
    .to_string()
}

/// "Sep 19, 2026 - 14:05 CEST", in local time where it can be had.
fn format_timestamp(ts: &str) -> String {
    let Ok(timestamp) = ts.parse::<Timestamp>() else {
        return ts.to_string();
    };
    let zone = jiff::tz::TimeZone::try_system().unwrap_or(jiff::tz::TimeZone::UTC);
    timestamp.to_zoned(zone).strftime("%b %-d, %Y - %H:%M %Z").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn slugs_round_trip() {
        for s in ServiceId::ALL {
            assert_eq!(ServiceId::from_slug(s.slug()), Some(s));
        }
        assert_eq!(ServiceId::from_slug("bitbucket"), None);
    }

    #[test]
    fn a_component_page_reads_only_its_component_and_incidents() {
        let resp = json!({
            "status": { "indicator": "major" },
            "components": [
                { "id": "a", "name": "claude.ai", "status": "major_outage" },
                { "id": "b", "name": "Claude Code", "status": "degraded_performance" },
            ],
            "incidents": [
                { "name": "Chat down", "impact": "major", "components": [{ "id": "a" }] },
                {
                    "name": "Slow CLI",
                    "impact": "minor",
                    "components": [{ "id": "b" }],
                    "incident_updates": [
                        { "status": "investigating", "body": "Looking", "created_at": "bad" }
                    ]
                },
            ]
        });
        let status = parse(ServiceId::Claude, &resp).expect("parsed");
        assert_eq!(status.health, Health::Degraded);
        assert_eq!(status.incidents.len(), 1);
        assert_eq!(status.incidents[0].name, "Slow CLI");
        assert_eq!(status.incidents[0].updates[0].body, "Looking");
    }

    #[test]
    fn codex_matches_a_renamed_component_by_prefix() {
        let resp = json!({
            "components": [
                { "id": "x", "name": "Codexical", "status": "major_outage" },
                { "id": "y", "name": "Codex API", "status": "operational" },
            ]
        });
        assert_eq!(parse(ServiceId::Codex, &resp).expect("parsed").health, Health::Operational);
    }

    #[test]
    fn a_missing_component_is_not_read_as_healthy() {
        let resp = json!({ "components": [{ "id": "a", "name": "Other", "status": "operational" }] });
        assert_eq!(parse(ServiceId::Claude, &resp), None);
    }

    #[test]
    fn a_whole_page_reads_its_indicator() {
        let resp = json!({ "status": { "indicator": "critical" }, "incidents": [{ "name": "Git down" }] });
        let status = parse(ServiceId::GitHub, &resp).expect("parsed");
        assert_eq!(status.health, Health::MajorOutage);
        assert_eq!(status.incidents.len(), 1);
    }

    #[test]
    fn status_io_reads_the_overall_code_and_incidents() {
        let resp = json!({
            "result": {
                "status_overall": { "status_code": 300 },
                "incidents": [{
                    "name": "CI delays",
                    "current_status": 400,
                    "messages": [{ "details": "Runners backed up", "state": "Investigating", "datetime": "" }]
                }]
            }
        });
        let status = parse(ServiceId::GitLab, &resp).expect("parsed");
        assert_eq!(status.health, Health::Degraded);
        assert_eq!(status.incidents[0].impact, "major");
        assert_eq!(status.incidents[0].updates[0].status, "Investigating");
        assert_eq!(parse(ServiceId::GitLab, &json!({})), None);
    }
}
