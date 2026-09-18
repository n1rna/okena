//! `extension.toml`: what an extension is, needs and may do.

use std::collections::HashSet;
use std::path::Path;

use okena_core::extension::{ExtConfigField, ExtPermissions, ExtRequiredTool};
use serde::Deserialize;

/// The file every extension folder holds.
pub const MANIFEST_FILE: &str = "extension.toml";

/// The prebuilt component that sits next to the manifest, when there is one.
pub const PREBUILT_WASM: &str = "extension.wasm";

/// The WIT interface versions this okena runs. A manifest names the one it
/// was built against in `api`.
pub const SUPPORTED_APIS: &[&str] = &["0.1"];

/// The shortest refresh interval okena honours.
pub const MIN_REFRESH_SECS: u64 = 5;

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    /// The WIT interface version it was built against.
    #[serde(default = "default_api")]
    pub api: String,
    /// How often okena refreshes it on its own; 0 means only on demand.
    #[serde(default)]
    pub refresh_interval_secs: u64,
    /// Present when the extension has a view of its own.
    #[serde(default)]
    pub view: Option<ViewSection>,
    #[serde(default)]
    pub permissions: ExtPermissions,
    #[serde(default)]
    pub requires: Vec<ExtRequiredTool>,
    #[serde(default)]
    pub config: Vec<ExtConfigField>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct ViewSection {
    /// The view's name in okena's navigation.
    pub title: String,
}

fn default_api() -> String {
    "0.1".to_string()
}

impl Manifest {
    pub fn parse(text: &str) -> Result<Self, String> {
        let manifest: Manifest =
            toml::from_str(text).map_err(|e| format!("{MANIFEST_FILE} is not valid: {e}"))?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn load(dir: &Path) -> Result<Self, String> {
        let path = dir.join(MANIFEST_FILE);
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        Self::parse(&text)
    }

    pub fn refresh_interval(&self) -> Option<std::time::Duration> {
        (self.refresh_interval_secs > 0).then(|| {
            std::time::Duration::from_secs(self.refresh_interval_secs.max(MIN_REFRESH_SECS))
        })
    }

    fn validate(&self) -> Result<(), String> {
        if !is_valid_id(&self.id) {
            return Err(format!(
                "id `{}` must be 1-64 lowercase letters, digits and dashes, starting with a letter or digit",
                self.id
            ));
        }
        if self.name.trim().is_empty() {
            return Err("name is empty".into());
        }
        semver::Version::parse(&self.version)
            .map_err(|e| format!("version `{}` is not a semantic version: {e}", self.version))?;
        if !SUPPORTED_APIS.contains(&self.api.as_str()) {
            return Err(format!(
                "api `{}` is not one this okena runs (it runs {}); update okena",
                self.api,
                SUPPORTED_APIS.join(", ")
            ));
        }

        for command in &self.permissions.commands {
            if command.trim().is_empty() || command.chars().any(char::is_whitespace) {
                return Err(format!(
                    "permissions.commands: `{command}` must be a program name without arguments"
                ));
            }
        }

        let mut keys = HashSet::new();
        for field in &self.config {
            if field.key.is_empty()
                || !field
                    .key
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
            {
                return Err(format!(
                    "config key `{}` must be lowercase letters, digits and underscores",
                    field.key
                ));
            }
            if !keys.insert(field.key.as_str()) {
                return Err(format!("config key `{}` is declared twice", field.key));
            }
        }

        for pattern in &self.permissions.paths {
            for key in config_placeholders(pattern) {
                if !keys.contains(key) {
                    return Err(format!(
                        "permissions.paths: `{pattern}` names `{{config.{key}}}`, which [[config]] does not declare"
                    ));
                }
            }
        }

        for tool in &self.requires {
            if tool.check.is_empty() {
                return Err(format!(
                    "requires `{}`: check needs a command, e.g. [\"{}\", \"--version\"]",
                    tool.name, tool.name
                ));
            }
            if let Some(min) = &tool.min_version
                && crate::deps::parse_version(min).is_none()
            {
                return Err(format!(
                    "requires `{}`: min_version `{min}` is not a version",
                    tool.name
                ));
            }
        }
        Ok(())
    }

    /// The configuration with defaults filled in, from what the user saved.
    pub fn effective_config(&self, saved: Option<&serde_json::Value>) -> serde_json::Value {
        let mut out = serde_json::Map::new();
        for field in &self.config {
            if let Some(default) = &field.default {
                out.insert(field.key.clone(), default.clone());
            }
        }
        if let Some(serde_json::Value::Object(saved)) = saved {
            for (key, value) in saved {
                if !value.is_null() {
                    out.insert(key.clone(), value.clone());
                }
            }
        }
        serde_json::Value::Object(out)
    }

    /// The required configuration fields that are still empty.
    pub fn missing_config(&self, config: &serde_json::Value) -> Vec<String> {
        self.config
            .iter()
            .filter(|field| field.required)
            .filter(|field| match config.get(&field.key) {
                None | Some(serde_json::Value::Null) => true,
                Some(serde_json::Value::String(s)) => s.trim().is_empty(),
                Some(_) => false,
            })
            .map(|field| field.key.clone())
            .collect()
    }
}

fn is_valid_id(id: &str) -> bool {
    let mut chars = id.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit())
        && id.len() <= 64
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// The `key`s of every `{config.key}` in a path pattern.
pub fn config_placeholders(pattern: &str) -> impl Iterator<Item = &str> {
    pattern.split("{config.").skip(1).filter_map(|rest| rest.split_once('}').map(|(key, _)| key))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL: &str = r#"
id = "cli-table"
name = "CLI table"
version = "0.2.0"
description = "Rows from a CLI"
refresh_interval_secs = 1

[view]
title = "Jobs"

[permissions]
commands = ["jq"]
paths = ["{config.data_file}", "~/.cache/jobs"]
start_agents = true

[[requires]]
name = "jq"
check = ["jq", "--version"]
min_version = "1.6"
install_hint = "brew install jq"

[[config]]
key = "data_file"
label = "Data file"
type = "path"
required = true

[[config]]
key = "limit"
label = "Limit"
type = "number"
default = 50
"#;

    #[test]
    fn a_full_manifest_parses() {
        let m = Manifest::parse(FULL).expect("parses");
        assert_eq!(m.id, "cli-table");
        assert_eq!(m.view.as_ref().map(|v| v.title.as_str()), Some("Jobs"));
        assert!(m.permissions.start_agents);
        assert_eq!(m.requires[0].install_hint, "brew install jq");
        assert_eq!(
            m.config[0].kind,
            okena_core::extension::ConfigFieldKind::Path
        );
        // Below the floor, the interval is raised to it.
        assert_eq!(
            m.refresh_interval(),
            Some(std::time::Duration::from_secs(MIN_REFRESH_SECS))
        );
    }

    #[test]
    fn config_defaults_fill_in_and_required_fields_are_reported() {
        let m = Manifest::parse(FULL).expect("parses");
        let config = m.effective_config(None);
        assert_eq!(config["limit"], 50);
        assert_eq!(m.missing_config(&config), vec!["data_file".to_string()]);

        let saved = serde_json::json!({ "data_file": "/tmp/jobs.json", "limit": null });
        let config = m.effective_config(Some(&saved));
        assert_eq!(config["limit"], 50, "a null falls back to the default");
        assert!(m.missing_config(&config).is_empty());
    }

    #[test]
    fn bad_manifests_say_what_is_wrong() {
        let cases = [
            (FULL.replace("cli-table", "Cli Table"), "id `Cli Table`"),
            (FULL.replace("0.2.0", "two"), "not a semantic version"),
            (
                FULL.replace("refresh_interval_secs", "api = \"9.0\"\nrefresh_interval_secs"),
                "api `9.0`",
            ),
            (FULL.replace("{config.data_file}", "{config.nope}"), "{config.nope}"),
            (FULL.replace("check = [\"jq\", \"--version\"]", "check = []"), "check needs a command"),
            (FULL.replace("commands = [\"jq\"]", "commands = [\"jq -r\"]"), "without arguments"),
            (FULL.replace("key = \"limit\"", "key = \"data_file\""), "declared twice"),
            (FULL.replace("min_version = \"1.6\"", "min_version = \"new\""), "not a version"),
            ("id = 3".to_string(), "not valid"),
        ];
        for (text, expected) in cases {
            let err = Manifest::parse(&text).expect_err(expected);
            assert!(err.contains(expected), "{err:?} should mention {expected:?}");
        }
    }
}
