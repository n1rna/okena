//! A stable colour for a thing, derived from its id.
//!
//! Lists of similar rows — agents, tasks — are easier to tell apart when each
//! one carries a faint colour of its own. The colour comes from an id that never
//! changes, so a row keeps it across restarts, re-sorts and renames, and the
//! same key gives the same colour in every view that uses it: a task and the
//! agent working on it match.

use gpui::{Hsla, hsla};

/// How many hues are handed out. Evenly spaced steps rather than any hue at
/// all: two rows a few degrees apart would carry different colours and still
/// look the same, which is the problem the colour is meant to solve.
const HUE_STEPS: u32 = 12;

const SATURATION: f32 = 0.65;
const LIGHTNESS: f32 = 0.55;

/// The hue for `key`, as a fraction of the colour wheel.
///
/// FNV-1a rather than `DefaultHasher`, whose output is allowed to change
/// between Rust releases.
pub fn identity_hue(key: &str) -> f32 {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in key.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    (hash % HUE_STEPS) as f32 / HUE_STEPS as f32
}

/// `key`'s colour at the given opacity.
pub fn identity_color(key: &str, alpha: f32) -> Hsla {
    hsla(identity_hue(key), SATURATION, LIGHTNESS, alpha)
}

#[cfg(test)]
mod tests {
    use super::identity_hue;

    #[test]
    fn a_key_keeps_its_hue() {
        assert_eq!(identity_hue("session-abc"), identity_hue("session-abc"));
        assert!((0.0..1.0).contains(&identity_hue("session-abc")));
    }

    #[test]
    fn keys_are_spread_over_more_than_one_hue() {
        let hues: std::collections::HashSet<u32> = (0..20)
            .map(|i| (identity_hue(&format!("session-{i}")) * 12.0).round() as u32)
            .collect();
        assert!(hues.len() > 4, "{hues:?}");
    }
}
