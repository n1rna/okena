//! The one order knowledge roots layer in (QBL-425).
//!
//! Since QBL-415 nobody picks a root for a template, a partial or a skill:
//! every healthy root is a layer, and the first one holding the file wins. What
//! was missing was a say in *which* root that is. The order was whatever
//! discovery happened to produce — registered stores first, in registry order,
//! then the projects' own folders — so a new root landed wherever discovery put
//! it and could not be moved.
//!
//! The order is now one saved list of root keys, top first, kept with the
//! user's settings (`harness.knowledge.order`). Only the list is a preference;
//! the checkout paths stay machine state in the registry (ADR-0003), so a
//! settings file synced between machines carries an order that means the same
//! thing on each and no paths that exist on only one.
//!
//! Three rules, and nothing else:
//!
//! 1. A root the list names sits where the list puts it.
//! 2. A root it doesn't name — one added since the order was last saved — goes
//!    to the bottom, keeping discovery order among its peers. It cannot beat
//!    anything already arranged until it is moved up deliberately.
//! 3. okena's own `okena-defaults` is always last and never in the list.
//!    It holds a copy of the compiled-in defaults, so anything above it is an
//!    override; putting it in the list would let it be dragged above the very
//!    layers meant to override it.
//!
//! Layering itself is unchanged and lives in `okena_knowledge::prompts`: this
//! module only decides what "in order" means. It sits in `okena-core` because
//! the daemon resolves through the order and the client arranges it, and one
//! rule implemented twice is one rule that will drift. Keys for roots that have since gone are
//! ignored here and dropped by [`normalize`] the next time the order is saved.

use crate::knowledge::KnowledgeRoot;

/// Where `root` sits: its band, then its place within the band.
///
/// The bands are what makes a new root land at the bottom rather than at the
/// top, and okena's defaults land below even that.
fn rank(root: &KnowledgeRoot, order: &[String]) -> (u8, usize) {
    if root.builtin {
        return (2, 0);
    }
    match order.iter().position(|key| *key == root.key) {
        Some(at) => (0, at),
        None => (1, 0),
    }
}

/// Put `roots` in the saved order.
///
/// Stable, so roots the order doesn't name keep the order discovery found them
/// in — which is the order they used to layer in, and the one thing a user who
/// has never arranged anything should not see change.
pub fn apply(roots: &mut [KnowledgeRoot], order: &[String]) {
    roots.sort_by_key(|root| rank(root, order));
}

/// The order as it should be saved, given the roots that exist.
///
/// Every root that takes part, in the order it now layers in, which drops keys
/// for roots that are no longer discovered — unregistered, or a project that
/// left the workspace. `okena-defaults` is left out because it is not part of
/// the order; an unhealthy root is kept, because it is still a root and its
/// checkout may well come back.
pub fn normalize(roots: &[KnowledgeRoot], order: &[String]) -> Vec<String> {
    let mut listed: Vec<&KnowledgeRoot> = roots.iter().filter(|root| !root.builtin).collect();
    listed.sort_by_key(|root| rank(root, order));
    listed.into_iter().map(|root| root.key.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::{apply, normalize};
    use crate::knowledge::{KnowledgeRoot, KnowledgeRootKind};

    fn root(key: &str) -> KnowledgeRoot {
        KnowledgeRoot {
            key: key.into(),
            kind: KnowledgeRootKind::Store,
            name: key.into(),
            path: format!("/roots/{key}"),
            store_id: None,
            description: None,
            remote: None,
            healthy: true,
            builtin: false,
            git: None,
            counts: Default::default(),
            used_by: Vec::new(),
            status: Vec::new(),
        }
    }

    fn defaults() -> KnowledgeRoot {
        KnowledgeRoot {
            builtin: true,
            ..root("store:okena-defaults")
        }
    }

    fn order(keys: &[&str]) -> Vec<String> {
        keys.iter().map(|k| (*k).to_string()).collect()
    }

    fn keys(roots: &[KnowledgeRoot]) -> Vec<&str> {
        roots.iter().map(|r| r.key.as_str()).collect()
    }

    #[test]
    fn the_saved_order_decides_and_moving_a_root_changes_who_is_on_top() {
        let mut roots = vec![root("store:a"), root("store:b"), root("store:c")];
        apply(&mut roots, &order(&["store:c", "store:a", "store:b"]));
        assert_eq!(keys(&roots), ["store:c", "store:a", "store:b"]);

        // The same roots, the other arrangement: the top is whoever the order
        // says, which is the whole point of the story.
        apply(&mut roots, &order(&["store:b", "store:c", "store:a"]));
        assert_eq!(keys(&roots), ["store:b", "store:c", "store:a"]);
    }

    #[test]
    fn a_root_the_order_does_not_name_goes_to_the_bottom_in_discovery_order() {
        // `new-1` and `new-2` were added after the order was last saved. They
        // must not beat an arranged root, and between themselves they keep the
        // order discovery produced.
        let mut roots = vec![
            root("store:new-1"),
            root("store:b"),
            root("store:new-2"),
            root("store:a"),
        ];
        apply(&mut roots, &order(&["store:a", "store:b"]));
        assert_eq!(
            keys(&roots),
            ["store:a", "store:b", "store:new-1", "store:new-2"]
        );
    }

    #[test]
    fn okena_defaults_is_last_however_the_order_reads() {
        // Including when something has put its key in the list: the defaults
        // hold a copy of the built-ins, so above the layers is exactly where
        // they must never be.
        let mut roots = vec![defaults(), root("store:a"), root("store:b")];
        apply(&mut roots, &order(&["store:okena-defaults", "store:b"]));
        assert_eq!(
            keys(&roots),
            ["store:b", "store:a", "store:okena-defaults"]
        );
    }

    #[test]
    fn an_empty_order_leaves_discovery_order_alone_apart_from_the_defaults() {
        // What a user who has never arranged anything sees: what they saw
        // before this story, with okena's own store moved off the top.
        let mut roots = vec![defaults(), root("store:a"), root("path:/repo")];
        apply(&mut roots, &[]);
        assert_eq!(keys(&roots), ["store:a", "path:/repo", "store:okena-defaults"]);
    }

    #[test]
    fn normalizing_drops_roots_that_are_gone_and_never_lists_the_defaults() {
        let roots = vec![root("store:b"), root("store:new"), defaults()];
        // `store:gone` was unregistered since the order was saved.
        let saved = order(&["store:gone", "store:b"]);
        assert_eq!(
            normalize(&roots, &saved),
            ["store:b", "store:new"],
            "the gone key drops out, the new root is written in at the bottom"
        );
    }

    #[test]
    fn an_unhealthy_root_keeps_its_place_in_the_saved_order() {
        // A checkout that is missing today is not a root that is gone: it is
        // still discovered, still listed, and losing its place would quietly
        // reshuffle the layers the moment a clone went walkabout.
        let broken = KnowledgeRoot {
            healthy: false,
            ..root("store:a")
        };
        let roots = vec![root("store:b"), broken];
        assert_eq!(
            normalize(&roots, &order(&["store:a", "store:b"])),
            ["store:a", "store:b"]
        );
    }
}
