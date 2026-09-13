//! Links between projects, read from their maps (ADR-0006).
//!
//! Two sources, merged into one list:
//!
//! - **matched** — one scanned project's `consumes` entry and another's
//!   `exposes` entry carry the same `type` and `name`;
//! - **listed** — a map's `links` names the other project. A multi-project
//!   scan writes each link into both maps.
//!
//! A link found both ways is *confirmed*; one only listed is *found by scan*.
//! Nothing here reads prose or code: only the manifests.

use okena_core::project_map::{
    InterfaceKind, LinkDirection, LinkSource, LinkedProject, MapStatus, ProjectLink, ProjectLinks,
    ProjectMap, ProjectMapState, UnmatchedConsume, UnresolvedLink,
};
use std::collections::{BTreeMap, BTreeSet};

/// One okena project and what reading its map found.
#[derive(Clone, Copy, Debug)]
pub struct MapInput<'a> {
    pub project_id: &'a str,
    /// The okena project's name.
    pub name: &'a str,
    pub state: &'a ProjectMapState,
}

/// Where one link was seen.
#[derive(Default)]
struct Seen<'a> {
    matched: bool,
    /// Projects whose map lists the link.
    listed_by: BTreeSet<&'a str>,
}

/// Match links across `projects`. Only valid maps take part; every project is
/// still listed, with its map's status, so a gap shows.
pub fn match_links(projects: &[MapInput<'_>]) -> ProjectLinks {
    let scanned: Vec<(&str, &ProjectMap)> = projects
        .iter()
        .filter_map(|p| p.state.map().map(|map| (p.project_id, map)))
        .collect();

    let mut exposers: BTreeMap<(InterfaceKind, &str), Vec<&str>> = BTreeMap::new();
    for (id, map) in &scanned {
        for exposed in &map.exposes {
            let name = exposed.name.trim();
            if !name.is_empty() {
                exposers.entry((exposed.kind, name)).or_default().push(id);
            }
        }
    }

    // Keyed consumer, provider, type, name: one link per interface per pair.
    let mut seen: BTreeMap<(&str, &str, InterfaceKind, &str), Seen> = BTreeMap::new();
    for (id, map) in &scanned {
        for consumed in &map.consumes {
            let name = consumed.name.trim();
            for provider in exposers.get(&(consumed.kind, name)).into_iter().flatten() {
                if provider != id {
                    seen.entry((id, provider, consumed.kind, name))
                        .or_default()
                        .matched = true;
                }
            }
        }
    }

    // A link names the other project by its map's `project.name`; the okena
    // project name is accepted too, for a map that was named differently.
    let resolve = |wanted: &str| -> Option<&str> {
        let wanted = wanted.trim();
        scanned
            .iter()
            .find(|(_, map)| map.project.name.trim() == wanted)
            .map(|(id, _)| *id)
            .or_else(|| {
                projects
                    .iter()
                    .find(|p| p.state.map().is_some() && p.name.trim() == wanted)
                    .map(|p| p.project_id)
            })
    };
    let mut unresolved = Vec::new();
    for (id, map) in &scanned {
        for link in &map.links {
            match resolve(&link.project) {
                Some(other) if other != *id => {
                    let (consumer, provider) = match link.direction {
                        LinkDirection::Uses => (*id, other),
                        LinkDirection::UsedBy => (other, *id),
                    };
                    seen.entry((consumer, provider, link.kind, link.name.trim()))
                        .or_default()
                        .listed_by
                        .insert(id);
                }
                _ => unresolved.push(UnresolvedLink {
                    project: id.to_string(),
                    link: link.clone(),
                }),
            }
        }
    }

    let links = seen
        .iter()
        .map(|((consumer, provider, kind, name), s)| ProjectLink {
            consumer: consumer.to_string(),
            provider: provider.to_string(),
            kind: *kind,
            name: name.to_string(),
            source: match (s.matched, s.listed_by.is_empty()) {
                (true, false) => LinkSource::Confirmed,
                (true, true) => LinkSource::Matched,
                (false, _) => LinkSource::FoundByScan,
            },
            listed_only_by: match s.listed_by.len() {
                1 => s.listed_by.first().map(|p| p.to_string()),
                _ => None,
            },
        })
        .collect();

    // A consume a listed link already accounts for is not unmatched, even when
    // no map exposes it under that name.
    let covered: BTreeSet<(&str, InterfaceKind, &str)> = seen
        .keys()
        .map(|(consumer, _, kind, name)| (*consumer, *kind, *name))
        .collect();
    let mut unmatched = Vec::new();
    for (id, map) in &scanned {
        for consumed in &map.consumes {
            if !covered.contains(&(*id, consumed.kind, consumed.name.trim())) {
                unmatched.push(UnmatchedConsume {
                    project: id.to_string(),
                    interface: consumed.clone(),
                });
            }
        }
    }

    ProjectLinks {
        projects: projects
            .iter()
            .map(|p| LinkedProject {
                project_id: p.project_id.to_string(),
                name: p.name.to_string(),
                status: MapStatus::from(p.state),
                map_name: p.state.map().map(|m| m.project.name.clone()),
            })
            .collect(),
        links,
        unmatched,
        unresolved,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(yaml: &str) -> ProjectMapState {
        match crate::project_map::parse(yaml, "test") {
            Ok(map) => ProjectMapState::Scanned { map: Box::new(map) },
            Err(problems) => panic!("{problems:?}"),
        }
    }

    fn map(name: &str, rest: &str) -> ProjectMapState {
        state(&format!(
            "version: 1\nproject:\n  name: {name}\n  description: d\n{rest}"
        ))
    }

    fn inputs<'a>(states: &'a [(&'a str, ProjectMapState)]) -> Vec<MapInput<'a>> {
        states
            .iter()
            .map(|(id, state)| MapInput {
                project_id: id,
                name: id,
                state,
            })
            .collect()
    }

    fn summary(links: &ProjectLinks) -> Vec<(String, String, String, LinkSource)> {
        links
            .links
            .iter()
            .map(|l| {
                (
                    l.consumer.clone(),
                    l.provider.clone(),
                    format!("{} {}", l.kind.id(), l.name),
                    l.source,
                )
            })
            .collect()
    }

    const BILLING: &str = "exposes:\n  - type: topic\n    name: billing.invoice-issued\n";
    const WORKER: &str = "consumes:\n  - type: topic\n    name: billing.invoice-issued\n";

    #[test]
    fn a_consume_matching_an_expose_is_a_matched_link() {
        let states = [
            ("billing", map("billing", BILLING)),
            ("worker", map("worker", WORKER)),
        ];
        let links = match_links(&inputs(&states));
        assert_eq!(
            summary(&links),
            [(
                "worker".into(),
                "billing".into(),
                "topic billing.invoice-issued".into(),
                LinkSource::Matched
            )]
        );
        assert!(links.unmatched.is_empty());
        assert_eq!(links.links[0].listed_only_by, None);
    }

    #[test]
    fn a_type_mismatch_is_no_link() {
        let states = [
            ("billing", map("billing", BILLING)),
            (
                "worker",
                map(
                    "worker",
                    "consumes:\n  - type: queue\n    name: billing.invoice-issued\n",
                ),
            ),
        ];
        let links = match_links(&inputs(&states));
        assert!(links.links.is_empty());
        assert_eq!(links.unmatched.len(), 1);
    }

    #[test]
    fn a_consume_nobody_exposes_is_unmatched_not_dropped() {
        let states = [("worker", map("worker", WORKER))];
        let links = match_links(&inputs(&states));
        assert!(links.links.is_empty());
        assert_eq!(links.unmatched[0].project, "worker");
        assert_eq!(links.unmatched[0].interface.name, "billing.invoice-issued");
    }

    #[test]
    fn two_projects_exposing_the_same_thing_both_link() {
        let states = [
            ("billing", map("billing", BILLING)),
            ("billing-v2", map("billing-v2", BILLING)),
            ("worker", map("worker", WORKER)),
        ];
        let providers: Vec<String> = match_links(&inputs(&states))
            .links
            .into_iter()
            .map(|l| l.provider)
            .collect();
        assert_eq!(providers, ["billing", "billing-v2"]);
    }

    #[test]
    fn a_link_listed_on_both_sides_of_a_match_is_confirmed() {
        let states = [
            (
                "billing",
                map(
                    "billing",
                    &format!(
                        "{BILLING}links:\n  - project: worker\n    direction: used_by\n    type: topic\n    name: billing.invoice-issued\n"
                    ),
                ),
            ),
            (
                "worker",
                map(
                    "worker",
                    &format!(
                        "{WORKER}links:\n  - project: billing\n    direction: uses\n    type: topic\n    name: billing.invoice-issued\n"
                    ),
                ),
            ),
        ];
        let links = match_links(&inputs(&states));
        assert_eq!(links.links.len(), 1);
        assert_eq!(links.links[0].source, LinkSource::Confirmed);
        assert_eq!(links.links[0].listed_only_by, None);
    }

    #[test]
    fn a_link_only_a_scan_found_is_kept_and_a_one_sided_one_is_flagged() {
        // No exposes or consumes to match: the scan saw a link the manifests'
        // interface names do not show.
        let states = [
            (
                "api",
                map(
                    "api",
                    "links:\n  - project: accounts\n    direction: uses\n    type: grpc\n    name: acme.accounts.v1.AccountService\n",
                ),
            ),
            ("accounts", map("accounts", "")),
        ];
        let links = match_links(&inputs(&states));
        assert_eq!(
            summary(&links),
            [(
                "api".into(),
                "accounts".into(),
                "grpc acme.accounts.v1.AccountService".into(),
                LinkSource::FoundByScan
            )]
        );
        assert_eq!(links.links[0].listed_only_by.as_deref(), Some("api"));
    }

    #[test]
    fn a_listed_link_covers_its_consume() {
        let states = [
            (
                "worker",
                map(
                    "worker",
                    &format!(
                        "{WORKER}links:\n  - project: billing\n    direction: uses\n    type: topic\n    name: billing.invoice-issued\n"
                    ),
                ),
            ),
            ("billing", map("billing", "")),
        ];
        let links = match_links(&inputs(&states));
        assert!(links.unmatched.is_empty(), "{:?}", links.unmatched);
        assert_eq!(links.links[0].source, LinkSource::FoundByScan);
    }

    #[test]
    fn a_link_to_a_project_nobody_scanned_is_unresolved() {
        let states = [
            (
                "api",
                map(
                    "api",
                    "links:\n  - project: ledger\n    direction: uses\n    type: http\n    name: ledger/v1\n",
                ),
            ),
            ("ledger", ProjectMapState::NotScanned),
        ];
        let links = match_links(&inputs(&states));
        assert!(links.links.is_empty());
        assert_eq!(links.unresolved.len(), 1);
        assert_eq!(links.unresolved[0].link.project, "ledger");
        let statuses: Vec<MapStatus> = links.projects.iter().map(|p| p.status).collect();
        assert_eq!(statuses, [MapStatus::Scanned, MapStatus::NotScanned]);
    }

    #[test]
    fn a_link_may_name_the_okena_project_when_the_map_is_named_otherwise() {
        let states = [
            (
                "worker",
                map(
                    "worker",
                    "links:\n  - project: billing-svc\n    direction: uses\n    type: topic\n    name: t\n",
                ),
            ),
            ("billing-svc", map("acme-billing", "")),
        ];
        let links = match_links(&inputs(&states));
        assert_eq!(links.links.len(), 1);
        assert_eq!(links.links[0].provider, "billing-svc");
    }
}
