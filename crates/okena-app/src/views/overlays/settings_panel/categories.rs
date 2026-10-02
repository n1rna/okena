#[derive(Clone, PartialEq)]
pub(in crate::views::overlays) enum SettingsCategory {
    General,
    Font,
    Terminal,
    Worktree,
    GitHub,
    Harness,
    /// The Library's origins of every type: knowledge, specs and freeform.
    Library,
    Tasks,
    Hooks,
    Extensions,
    PairedDevices,
    /// Dynamic category for an extension's own settings (keyed by extension ID).
    Extension(String),
}

impl SettingsCategory {
    pub(super) fn label(&self) -> &str {
        match self {
            Self::General => "General",
            Self::Font => "Font",
            Self::Terminal => "Terminal",
            Self::Worktree => "Worktree",
            Self::GitHub => "GitHub",
            Self::Harness => "Harness",
            Self::Library => "Library",
            Self::Tasks => "Tasks",
            Self::Hooks => "Hooks",
            Self::Extensions => "Extensions",
            Self::PairedDevices => "Devices",
            Self::Extension(_) => "", // label provided dynamically from registry
        }
    }

    pub(super) fn all() -> &'static [SettingsCategory] {
        &[
            Self::General,
            Self::Font,
            Self::Terminal,
            Self::Worktree,
            Self::GitHub,
            Self::Harness,
            Self::Library,
            Self::Tasks,
            Self::Hooks,
            Self::Extensions,
            Self::PairedDevices,
        ]
    }

    /// Stable id used to open the panel on a given page from elsewhere.
    ///
    /// A string crosses crate boundaries that the enum cannot: the harness
    /// views live in this crate but request the page through the shared
    /// request broker, which does not know this type.
    pub(super) fn slug(&self) -> &str {
        match self {
            Self::General => "general",
            Self::Font => "font",
            Self::Terminal => "terminal",
            Self::Worktree => "worktree",
            Self::GitHub => "github",
            Self::Harness => "harness",
            Self::Library => "library",
            Self::Tasks => "tasks",
            Self::Hooks => "hooks",
            Self::Extensions => "extensions",
            Self::PairedDevices => "devices",
            Self::Extension(id) => id,
        }
    }

    /// Resolve a slug back to a page. Unknown slugs open the default page
    /// rather than failing — a stale link should still open settings.
    ///
    /// `specs` and `knowledge` were the two pages Library replaced (QBL-440);
    /// a link to either opens it.
    pub(super) fn from_slug(slug: &str) -> Option<SettingsCategory> {
        match slug {
            "specs" | "knowledge" => Some(Self::Library),
            _ => Self::all().iter().find(|c| c.slug() == slug).cloned(),
        }
    }

    /// Categories available in project mode (only hooks for now)
    pub(super) fn project_categories() -> &'static [SettingsCategory] {
        &[Self::Hooks]
    }
}

#[cfg(test)]
mod tests {
    use super::SettingsCategory;

    #[test]
    fn library_is_the_one_page_where_specs_and_knowledge_were() {
        let labels: Vec<&str> = SettingsCategory::all().iter().map(|c| c.label()).collect();
        assert!(labels.contains(&"Library"));
        assert!(!labels.contains(&"Specs") && !labels.contains(&"Knowledge"), "{labels:?}");
        // Sits where the two pages did, between Harness and Tasks.
        let at = |name: &str| labels.iter().position(|l| *l == name).expect(name);
        assert_eq!(at("Library"), at("Harness") + 1);
        assert_eq!(at("Tasks"), at("Library") + 1);
    }

    #[test]
    fn a_link_to_either_old_page_opens_library() {
        for slug in ["library", "specs", "knowledge"] {
            assert!(
                SettingsCategory::from_slug(slug) == Some(SettingsCategory::Library),
                "{slug}"
            );
        }
        assert!(SettingsCategory::from_slug("nope").is_none());
    }

    #[test]
    fn every_page_is_found_by_its_own_slug() {
        for page in SettingsCategory::all() {
            assert!(SettingsCategory::from_slug(page.slug()) == Some(page.clone()));
        }
    }
}
