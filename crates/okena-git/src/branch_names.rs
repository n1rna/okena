use std::collections::HashSet;
use std::path::Path;

#[derive(Clone, Copy)]
enum Gender {
    Masculine,
    Feminine,
    Neuter,
}

struct BakedGood {
    name: &'static str,
    gender: Gender,
}

const GOODS: &[BakedGood] = &[
    BakedGood {
        name: "rohlik",
        gender: Gender::Masculine,
    },
    BakedGood {
        name: "houska",
        gender: Gender::Feminine,
    },
    BakedGood {
        name: "kolac",
        gender: Gender::Masculine,
    },
    BakedGood {
        name: "veka",
        gender: Gender::Feminine,
    },
    BakedGood {
        name: "chleb",
        gender: Gender::Masculine,
    },
    BakedGood {
        name: "buchta",
        gender: Gender::Feminine,
    },
    BakedGood {
        name: "kobliha",
        gender: Gender::Feminine,
    },
    BakedGood {
        name: "strudl",
        gender: Gender::Masculine,
    },
    BakedGood {
        name: "mazanec",
        gender: Gender::Masculine,
    },
    BakedGood {
        name: "vanocka",
        gender: Gender::Feminine,
    },
    BakedGood {
        name: "trdlo",
        gender: Gender::Neuter,
    },
    BakedGood {
        name: "trdelnik",
        gender: Gender::Masculine,
    },
    BakedGood {
        name: "loupak",
        gender: Gender::Masculine,
    },
    BakedGood {
        name: "makovec",
        gender: Gender::Masculine,
    },
    BakedGood {
        name: "zavin",
        gender: Gender::Masculine,
    },
    BakedGood {
        name: "kremrole",
        gender: Gender::Feminine,
    },
    BakedGood {
        name: "venecek",
        gender: Gender::Masculine,
    },
    BakedGood {
        name: "rakvicka",
        gender: Gender::Feminine,
    },
    BakedGood {
        name: "laskonka",
        gender: Gender::Feminine,
    },
    BakedGood {
        name: "medovnik",
        gender: Gender::Masculine,
    },
    BakedGood {
        name: "bublanina",
        gender: Gender::Feminine,
    },
    BakedGood {
        name: "pernik",
        gender: Gender::Masculine,
    },
    BakedGood {
        name: "knedlik",
        gender: Gender::Masculine,
    },
    BakedGood {
        name: "palacinka",
        gender: Gender::Feminine,
    },
    BakedGood {
        name: "babovka",
        gender: Gender::Feminine,
    },
    BakedGood {
        name: "povidlak",
        gender: Gender::Masculine,
    },
    BakedGood {
        name: "vdolek",
        gender: Gender::Masculine,
    },
    BakedGood {
        name: "bochanek",
        gender: Gender::Masculine,
    },
    BakedGood {
        name: "kolatek",
        gender: Gender::Masculine,
    },
    BakedGood {
        name: "zemle",
        gender: Gender::Feminine,
    },
    BakedGood {
        name: "paska",
        gender: Gender::Feminine,
    },
    BakedGood {
        name: "pletenak",
        gender: Gender::Masculine,
    },
    BakedGood {
        name: "orechovec",
        gender: Gender::Masculine,
    },
    BakedGood {
        name: "tvarohac",
        gender: Gender::Masculine,
    },
    BakedGood {
        name: "jablecnak",
        gender: Gender::Masculine,
    },
    BakedGood {
        name: "svestkac",
        gender: Gender::Masculine,
    },
    BakedGood {
        name: "linecak",
        gender: Gender::Masculine,
    },
    BakedGood {
        name: "vetrnik",
        gender: Gender::Masculine,
    },
];

/// (stem, masculine_suffix, feminine_suffix, neuter_suffix)
const ADJECTIVE_STEMS: &[(&str, &str, &str, &str)] = &[
    ("velk", "y", "a", "e"),
    ("mal", "y", "a", "e"),
    ("zlat", "y", "a", "e"),
    ("cerstv", "y", "a", "e"),
    ("sladk", "y", "a", "e"),
    ("tezk", "y", "a", "e"),
    ("lehk", "y", "a", "e"),
    ("hork", "y", "a", "e"),
    ("divok", "y", "a", "e"),
    ("rychl", "y", "a", "e"),
];

fn adjective_for(
    stem: &str,
    suffix_m: &str,
    suffix_f: &str,
    suffix_n: &str,
    good: &BakedGood,
) -> String {
    let suffix = match good.gender {
        Gender::Masculine => suffix_m,
        Gender::Feminine => suffix_f,
        Gender::Neuter => suffix_n,
    };
    format!("{}{}", stem, suffix)
}

/// Prefix for quick-created worktrees. They carry no ticket to say what kind
/// of work they are, so they read as chores, matching the commit style.
const PREFIX: &str = "chore";

/// Generate a unique branch name like `chore/rohlik` that doesn't collide
/// with existing branches or worktree branches.
///
/// **Blocking I/O**: spawns git subprocesses. Must be called off the main
/// thread (e.g., via `smol::unblock`).
pub fn generate_branch_name(repo_path: &Path) -> String {
    pick_branch_name(&collect_taken_branches(repo_path))
}

fn pick_branch_name(taken: &HashSet<String>) -> String {
    // Shuffle goods and adjectives so the generated name feels random
    let mut good_idx: Vec<usize> = (0..GOODS.len()).collect();
    let mut adj_idx: Vec<usize> = (0..ADJECTIVE_STEMS.len()).collect();
    shuffle(&mut good_idx);
    shuffle(&mut adj_idx);

    // Phase 1: try plain goods
    for &i in &good_idx {
        let candidate = format!("{}/{}", PREFIX, GOODS[i].name);
        if !taken.contains(&candidate) {
            return candidate;
        }
    }

    // Phase 2: try adjective+good combos
    for &ai in &adj_idx {
        let (stem, sm, sf, sn) = ADJECTIVE_STEMS[ai];
        for &i in &good_idx {
            let good = &GOODS[i];
            let adj = adjective_for(stem, sm, sf, sn, good);
            let candidate = format!("{}/{}-{}", PREFIX, adj, good.name);
            if !taken.contains(&candidate) {
                return candidate;
            }
        }
    }

    // Phase 3: numeric suffix fallback (practically unreachable — Phase 1 covers 38,
    // Phase 2 covers 380 combos, so 418+ branches must already exist under this prefix)
    for suffix_num in 2u32..1000 {
        for &ai in &adj_idx {
            let (stem, sm, sf, sn) = ADJECTIVE_STEMS[ai];
            for &i in &good_idx {
                let good = &GOODS[i];
                let adj = adjective_for(stem, sm, sf, sn, good);
                let candidate = format!("{}/{}-{}-{}", PREFIX, adj, good.name, suffix_num);
                if !taken.contains(&candidate) {
                    return candidate;
                }
            }
        }
    }

    // Fallback: UUID-based name (practically unreachable)
    format!("{}/worktree-{}", PREFIX, uuid::Uuid::new_v4())
}

fn collect_taken_branches(repo_path: &Path) -> HashSet<String> {
    // list_branches and get_worktree_branches are independent git commands —
    // run them in parallel to halve the latency.
    #[allow(
        clippy::expect_used,
        reason = "scoped worker panic re-raised by the orchestrator is the intended behavior"
    )]
    let (branches, wt_branches) = std::thread::scope(|s| {
        let b = s.spawn(|| super::repository::list_branches(repo_path));
        let w = s.spawn(|| super::repository::get_worktree_branches(repo_path));
        (
            b.join().expect("branch listing thread panicked"),
            w.join().expect("worktree branch listing thread panicked"),
        )
    });
    let mut taken: HashSet<String> = branches.into_iter().collect();
    taken.extend(wt_branches);
    taken
}

/// Simple Fisher-Yates shuffle using system time as seed
fn shuffle(indices: &mut [usize]) {
    use std::time::SystemTime;
    let seed = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(42);
    // Mix in the process ID so that two calls in the same nanosecond (e.g. in
    // parallel tests) are unlikely to produce the same permutation.
    let seed = seed.wrapping_add(std::process::id() as u64);
    let mut rng = seed;
    for i in (1..indices.len()).rev() {
        // Simple xorshift64
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        let j = (rng as usize) % (i + 1);
        indices.swap(i, j);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_adjective_gender_agreement() {
        let m_good = BakedGood {
            name: "rohlik",
            gender: Gender::Masculine,
        };
        let f_good = BakedGood {
            name: "houska",
            gender: Gender::Feminine,
        };
        let n_good = BakedGood {
            name: "trdlo",
            gender: Gender::Neuter,
        };

        assert_eq!(adjective_for("velk", "y", "a", "e", &m_good), "velky");
        assert_eq!(adjective_for("velk", "y", "a", "e", &f_good), "velka");
        assert_eq!(adjective_for("velk", "y", "a", "e", &n_good), "velke");
    }

    #[test]
    fn names_are_chores_without_a_username() {
        let name = pick_branch_name(&HashSet::new());
        let good = name.strip_prefix("chore/").expect("chore prefix");
        assert!(GOODS.iter().any(|g| g.name == good), "got {name}");
    }

    #[test]
    fn taken_names_are_skipped() {
        let mut taken: HashSet<String> =
            GOODS.iter().map(|g| format!("chore/{}", g.name)).collect();
        taken.remove("chore/rohlik");
        assert_eq!(pick_branch_name(&taken), "chore/rohlik");

        // Every plain good taken: falls through to adjective combos.
        taken.insert("chore/rohlik".into());
        let name = pick_branch_name(&taken);
        assert!(
            name.starts_with("chore/") && !taken.contains(&name),
            "got {name}"
        );
    }

    #[test]
    fn test_generate_avoids_collisions() {
        // We can't easily call generate_branch_name without a real repo,
        // but we can test the collision logic by checking that the goods list
        // and adjective stems are well-formed.
        assert_eq!(GOODS.len(), 38);
        assert_eq!(ADJECTIVE_STEMS.len(), 10);

        // Verify all goods have non-empty names
        for good in GOODS {
            assert!(!good.name.is_empty());
        }
    }
}
