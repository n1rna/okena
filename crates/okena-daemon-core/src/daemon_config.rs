//! GPUI-free settings & theme handlers for the headless daemon.
//!
//! This is the headless counterpart to the desktop app's
//! `okena-app/src/app/remote_config.rs`. Both now share the same logic via
//! [`okena_app_core::remote_config`]; this module only supplies the daemon's
//! [`ConfigBackend`] impl (a shared `Arc<parking_lot::Mutex<AppSettings>>`
//! backing store) and thin method wrappers over the shared functions.
//!
//! The data-vs-presentation split this migration follows: the daemon owns the
//! theme **preference** (the data — `theme_mode` + `custom_theme_id`, persisted
//! to settings.json) and the custom-theme files on disk. It does NOT own an
//! `AppTheme` entity, because applying colors to pixels is the client's job.
//! The daemon still publishes the palette used for terminal color queries.
//! Auto uses the local desktop's reported appearance, with a dark fallback
//! until a desktop connects. Appearance is transient, not a saved preference.
//!
//! State is shared through a single `Arc<parking_lot::Mutex<AppSettings>>`,
//! loaded once at daemon startup via [`load_settings`]. [`DaemonConfig`] is the
//! write path; other daemon code reads the same `Arc`.
//!
//! [`load_settings`]: okena_workspace::persistence::load_settings

use std::sync::Arc;

use okena_app_core::remote_config::{self, ConfigBackend};
use okena_core::api::CommandResult;
use okena_theme::custom::load_custom_themes;
use okena_theme::{
    DARK_THEME, HIGH_CONTRAST_THEME, LIGHT_THEME, PASTEL_DARK_THEME, ThemeColors, ThemeMode,
};
use okena_workspace::persistence::AppSettings;
use okena_workspace::settings::save_settings;
use parking_lot::Mutex;
use serde_json::Value;

type SettingsPersister = Arc<dyn Fn(&AppSettings) -> Result<(), String> + Send + Sync>;

/// GPUI-free settings & theme handler backed by a shared
/// `Arc<parking_lot::Mutex<AppSettings>>`.
pub struct DaemonConfig {
    settings: Arc<Mutex<AppSettings>>,
    persist_settings: SettingsPersister,
    system_is_dark: bool,
    publish_palette: Arc<dyn Fn(ThemeColors) + Send + Sync>,
}

impl DaemonConfig {
    /// Build the handler over the daemon's single shared settings cell.
    ///
    /// The daemon loads settings once at startup (via `load_settings()`) into
    /// this `Arc<Mutex<AppSettings>>`; this struct is the write path while
    /// other daemon code reads the same `Arc`.
    pub fn new(settings: Arc<Mutex<AppSettings>>) -> Self {
        Self {
            settings,
            persist_settings: Arc::new(|settings| {
                save_settings(settings).map_err(|error| error.to_string())
            }),
            system_is_dark: true,
            publish_palette: Arc::new(okena_terminal::terminal::set_process_palette),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_persistence(
        settings: Arc<Mutex<AppSettings>>,
        persist_settings: SettingsPersister,
    ) -> Self {
        Self {
            settings,
            persist_settings,
            system_is_dark: true,
            publish_palette: Arc::new(okena_terminal::terminal::set_process_palette),
        }
    }

    /// Update Auto's resolved appearance without changing the saved theme.
    pub fn set_system_appearance(&mut self, is_dark: bool) -> CommandResult {
        self.system_is_dark = is_dark;
        self.refresh_palette();
        CommandResult::Ok(None)
    }

    fn refresh_palette(&mut self) {
        let settings = self.settings.lock().clone();
        let colors =
            self.active_theme_colors(settings.theme_mode, settings.custom_theme_id.as_deref());
        (self.publish_palette)(colors);
    }

    /// Return the full current settings as JSON.
    pub fn get_settings(&mut self) -> CommandResult {
        remote_config::get_settings(self)
    }

    /// Deep-merge `patch` into the current settings, validate by
    /// re-deserializing, persist, then replace the held value.
    ///
    /// Unlike the GUI there is no settings observer here, so changes to the
    /// `remote_*` fields do NOT hot-restart the remote server — they apply on
    /// the next daemon launch. On a save failure the held value is left
    /// unchanged.
    pub fn set_settings(&mut self, patch: Value) -> CommandResult {
        remote_config::set_settings(self, patch)
    }

    /// Apply `edit` to the held settings and persist the result.
    ///
    /// The write path for changes that are not a JSON patch — spaces, whose
    /// rules live in `okena_workspace::spaces` and span more than one key. On
    /// a save failure the held value is rolled back, so the daemon's settings
    /// and `settings.json` never disagree.
    pub fn edit_settings<T>(
        &mut self,
        edit: impl FnOnce(&mut AppSettings) -> Result<T, String>,
    ) -> Result<T, String> {
        let before = self.settings.lock().clone();
        let (value, next) = {
            let mut held = self.settings.lock();
            let value = edit(&mut held)?;
            (value, held.clone())
        };
        if let Err(e) = (self.persist_settings)(&next) {
            *self.settings.lock() = before;
            return Err(e);
        }
        Ok(value)
    }

    /// Validate a patch without persisting it or changing the shared settings.
    pub fn preview_settings(&self, patch: Value) -> Result<AppSettings, String> {
        remote_config::preview_settings_patch(&self.settings.lock(), patch)
    }

    /// Persist and publish settings that were already validated by [`Self::preview_settings`].
    pub fn store_prevalidated_settings(&mut self, settings: &AppSettings) -> CommandResult {
        remote_config::store_prevalidated_settings(self, settings)
    }

    /// List built-in + custom themes, flagging the active one.
    pub fn get_themes(&mut self) -> CommandResult {
        remote_config::get_themes(self)
    }

    /// Return a theme as an editable custom-theme blob (the active theme when
    /// `id` is None).
    pub fn get_theme(&mut self, id: Option<String>) -> CommandResult {
        remote_config::get_theme(self, id)
    }

    /// Activate a theme: a built-in mode or a custom theme id. Persists the
    /// preference to settings.json (there is no `AppTheme` to update).
    pub fn set_theme(&mut self, id: String) -> CommandResult {
        remote_config::set_theme(self, id)
    }

    /// Write a custom theme JSON file (a full `CustomThemeConfig`) and, when
    /// `activate`, switch the persisted preference to it.
    pub fn save_custom_theme(
        &mut self,
        id: String,
        config: Value,
        activate: bool,
    ) -> CommandResult {
        remote_config::save_custom_theme(self, id, config, activate)
    }
}

impl ConfigBackend for DaemonConfig {
    fn load_settings(&mut self) -> AppSettings {
        self.settings.lock().clone()
    }

    fn store_settings(&mut self, new: &AppSettings) -> Result<(), String> {
        // Persist to disk first; only replace the held value on success so a
        // save failure leaves the in-memory settings unchanged.
        (self.persist_settings)(new)?;
        let (hosts_changed, gh_path_changed) = {
            let mut held = self.settings.lock();
            let changed = (
                held.github_enterprise_hosts != new.github_enterprise_hosts,
                held.gh_path != new.gh_path,
            );
            *held = new.clone();
            changed
        };
        // Only on a change: both are process-wide, and a store that leaves
        // them alone has nothing to tell the GitHub code.
        if hosts_changed {
            okena_git::repository::set_enterprise_hosts(&new.github_enterprise_hosts);
        }
        if gh_path_changed {
            okena_git::repository::set_gh_path(new.gh_path.as_deref());
        }
        self.refresh_palette();
        Ok(())
    }

    fn apply_active_theme(&mut self, mode: ThemeMode, custom_colors: Option<ThemeColors>) {
        // Headless: no live theme surface to update (the preference is already
        // persisted), but the daemon's terminals answer OSC color queries from
        // the process palette — keep it in sync with the active theme.
        let colors = match custom_colors {
            Some(colors) => colors,
            None => self.active_theme_colors(mode, None),
        };
        (self.publish_palette)(colors);
    }

    fn active_theme_colors(&mut self, mode: ThemeMode, custom_id: Option<&str>) -> ThemeColors {
        // The desktop reports system appearance without adding a windowing
        // dependency to the daemon. Explicit themes ignore that report.
        match mode {
            ThemeMode::Auto if !self.system_is_dark => LIGHT_THEME,
            ThemeMode::Dark | ThemeMode::Auto => DARK_THEME,
            ThemeMode::Light => LIGHT_THEME,
            ThemeMode::PastelDark => PASTEL_DARK_THEME,
            ThemeMode::HighContrast => HIGH_CONTRAST_THEME,
            ThemeMode::Custom => {
                let target = custom_id.map(|cid| format!("custom:{cid}"));
                match target.and_then(|t| load_custom_themes().into_iter().find(|(i, _)| i.id == t))
                {
                    Some((_, colors)) => colors,
                    // Custom mode but no resolvable custom theme: fall back to
                    // dark so we still return an editable blob.
                    None => DARK_THEME,
                }
            }
        }
    }
}

/// Return a defaults instance of the settings — every key with its default
/// value, as a de-facto schema agents can read to discover available keys.
pub fn get_settings_schema() -> CommandResult {
    remote_config::get_settings_schema()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::default_settings;
    use serde_json::json;

    fn config_with(settings: AppSettings) -> DaemonConfig {
        DaemonConfig::new(Arc::new(Mutex::new(settings)))
    }

    fn config_with_persistence(
        settings: AppSettings,
        persist: SettingsPersister,
    ) -> DaemonConfig {
        DaemonConfig::with_persistence(Arc::new(Mutex::new(settings)), persist)
    }

    #[test]
    fn editing_settings_persists_the_result() {
        let saved: Arc<Mutex<Option<AppSettings>>> = Arc::new(Mutex::new(None));
        let sink = saved.clone();
        let mut config = config_with_persistence(
            default_settings(),
            Arc::new(move |s: &AppSettings| {
                *sink.lock() = Some(s.clone());
                Ok(())
            }),
        );

        let space = config
            .edit_settings(|s| {
                okena_workspace::spaces::create(s, "Client A", None, Default::default())
            })
            .expect("created");
        assert_eq!(space.id, "client-a");
        let written = saved.lock().clone().expect("persisted");
        assert!(written.space("client-a").is_some());
        assert!(config.settings.lock().space("client-a").is_some());
    }

    #[test]
    fn a_failed_save_leaves_the_held_settings_untouched() {
        // Otherwise the daemon would show a space that settings.json does not
        // have, and the next restart would lose it under the user.
        let mut config = config_with_persistence(
            default_settings(),
            Arc::new(|_: &AppSettings| Err("disk full".to_string())),
        );
        let before = config.settings.lock().spaces.len();
        let result = config.edit_settings(|s| {
            okena_workspace::spaces::create(s, "Client A", None, Default::default())
        });
        assert_eq!(result.err().as_deref(), Some("disk full"));
        assert_eq!(config.settings.lock().spaces.len(), before);
        assert!(config.settings.lock().space("client-a").is_none());
    }

    #[test]
    fn a_refused_edit_writes_nothing() {
        let saved: Arc<Mutex<Option<AppSettings>>> = Arc::new(Mutex::new(None));
        let sink = saved.clone();
        let mut config = config_with_persistence(
            default_settings(),
            Arc::new(move |s: &AppSettings| {
                *sink.lock() = Some(s.clone());
                Ok(())
            }),
        );
        let result = config.edit_settings(|s| okena_workspace::spaces::remove(s, "default"));
        assert!(result.is_err(), "Default cannot be deleted");
        assert!(saved.lock().is_none(), "a refused edit must not save");
    }


    #[test]
    fn a_github_host_added_in_settings_is_polled_until_it_is_removed() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path();
        for args in [
            &["init", "-q"][..],
            &[
                "remote",
                "add",
                "origin",
                "https://ghe.settings-only.example/team/app.git",
            ],
        ] {
            let status = std::process::Command::new("git")
                .args(args)
                .current_dir(repo)
                .status()
                .expect("git runs");
            assert!(status.success());
        }
        let settings = Arc::new(Mutex::new(default_settings()));
        let mut cfg = DaemonConfig::with_persistence(settings.clone(), Arc::new(|_| Ok(())));
        assert!(!okena_git::repository::has_github_remote(repo));

        assert!(matches!(
            cfg.set_settings(json!({"github_enterprise_hosts": ["ghe.settings-only.example"]})),
            CommandResult::Ok(_)
        ));
        assert!(okena_git::repository::has_github_remote(repo));

        assert!(matches!(
            cfg.set_settings(json!({"github_enterprise_hosts": []})),
            CommandResult::Ok(_)
        ));
        assert!(settings.lock().github_enterprise_hosts.is_empty());
        assert!(!okena_git::repository::has_github_remote(repo));
    }

    #[cfg(unix)]
    #[test]
    fn the_gh_path_set_in_settings_is_the_gh_that_runs_until_cleared() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().expect("tempdir");
        let gh = tmp.path().join("gh");
        std::fs::write(&gh, "#!/bin/sh\nexit 0\n").expect("write gh");
        std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).expect("chmod");

        let settings = Arc::new(Mutex::new(default_settings()));
        let mut cfg = DaemonConfig::with_persistence(settings.clone(), Arc::new(|_| Ok(())));

        // A directory holding gh works as well as the binary itself.
        assert!(matches!(
            cfg.set_settings(json!({"gh_path": tmp.path().to_str().unwrap()})),
            CommandResult::Ok(_)
        ));
        assert_eq!(okena_git::repository::resolved_gh_path(), Some(gh.clone()));

        assert!(matches!(
            cfg.set_settings(json!({"gh_path": null})),
            CommandResult::Ok(_)
        ));
        assert_eq!(settings.lock().gh_path, None);
        assert_ne!(okena_git::repository::resolved_gh_path(), Some(gh));
    }

    #[test]
    fn terminal_palette_tracks_settings_and_transient_system_appearance() {
        let settings = Arc::new(Mutex::new(default_settings()));
        settings.lock().theme_mode = ThemeMode::Auto;
        let mut cfg = DaemonConfig::with_persistence(settings.clone(), Arc::new(|_| Ok(())));
        let published = Arc::new(Mutex::new(Vec::new()));
        let captured = published.clone();
        cfg.publish_palette = Arc::new(move |colors| {
            captured
                .lock()
                .push((colors.term_foreground, colors.term_background));
        });

        // Headless fallback, then the light desktop's initial report.
        cfg.refresh_palette();
        cfg.set_system_appearance(false);
        assert_eq!(settings.lock().theme_mode, ThemeMode::Auto);

        // The GUI uses SetSettings, not SetTheme. Explicit themes must win
        // over OS appearance, including an OS change while Dark is selected.
        assert!(matches!(
            cfg.set_settings(json!({"theme_mode": "dark"})),
            CommandResult::Ok(_)
        ));
        cfg.set_system_appearance(true);
        assert!(matches!(
            cfg.set_settings(json!({"theme_mode": "light"})),
            CommandResult::Ok(_)
        ));
        assert!(matches!(
            cfg.set_settings(json!({"theme_mode": "auto"})),
            CommandResult::Ok(_)
        ));
        cfg.set_system_appearance(false);

        let dark = (DARK_THEME.term_foreground, DARK_THEME.term_background);
        let light = (LIGHT_THEME.term_foreground, LIGHT_THEME.term_background);
        assert_eq!(
            *published.lock(),
            vec![dark, light, dark, dark, light, dark, light]
        );
    }

    #[test]
    fn failed_settings_save_keeps_terminal_palette_and_preference() {
        let mut settings = default_settings();
        settings.theme_mode = ThemeMode::Dark;
        let settings = Arc::new(Mutex::new(settings));
        let mut cfg = DaemonConfig::with_persistence(
            settings.clone(),
            Arc::new(|_| Err("disk unavailable".into())),
        );
        cfg.publish_palette = Arc::new(|_| panic!("failed save must not publish a palette"));
        assert!(matches!(
            cfg.set_settings(json!({"theme_mode": "light"})),
            CommandResult::Err(_)
        ));
        assert_eq!(settings.lock().theme_mode, ThemeMode::Dark);
    }

    #[test]
    fn get_settings_returns_held_value_round_trips() {
        let mut settings = default_settings();
        settings.font_size = 17.5;
        settings.font_family = "Fira Code".to_string();
        let mut cfg = config_with(settings);

        match cfg.get_settings() {
            CommandResult::Ok(Some(v)) => {
                assert_eq!(v["font_size"], json!(17.5));
                assert_eq!(v["font_family"], json!("Fira Code"));
                // Round-trips back into AppSettings.
                let back: AppSettings = serde_json::from_value(v).expect("round-trip");
                assert_eq!(back.font_size, 17.5);
                assert_eq!(back.font_family, "Fira Code");
            }
            other => panic!("expected Ok(Some), got {other:?}"),
        }
    }

    #[test]
    fn get_settings_schema_contains_expected_keys() {
        match get_settings_schema() {
            CommandResult::Ok(Some(v)) => {
                let obj = v.as_object().expect("schema is an object");
                assert!(obj.contains_key("font_size"));
                assert!(obj.contains_key("theme_mode"));
                assert!(obj.contains_key("font_family"));
                // The schema deserializes back into AppSettings (it IS the
                // defaults instance).
                serde_json::from_value::<AppSettings>(v).expect("schema round-trips");
            }
            other => panic!("expected Ok(Some), got {other:?}"),
        }
    }
}
