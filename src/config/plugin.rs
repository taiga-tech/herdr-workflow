//! 利用者固有のプラグイン全体設定(`$HERDR_PLUGIN_CONFIG_DIR/config.yaml`)。
//!
//! workflow設定とは信頼境界を分け、ファイルがない場合だけ組み込み既定値を使う。
//! 存在するファイルの読み取り・構文・値エラーは呼び出し元へ返す。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const CONFIG_DIR_ENV: &str = "HERDR_PLUGIN_CONFIG_DIR";
pub const CONFIG_FILE_NAME: &str = "config.yaml";
pub const DEFAULT_MAX_GRID_DIMENSION: u32 = 64;
pub const MAX_CONFIGURABLE_GRID_DIMENSION: u32 = 64;
pub const DEFAULT_MAX_PANES_PER_TAB: u32 = 32;
pub const MAX_CONFIGURABLE_PANES_PER_TAB: u32 = 32;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(try_from = "RawPluginConfig", rename_all = "camelCase")]
pub struct PluginConfig {
    version: u32,
    #[serde(default)]
    limits: LimitsConfig,
}

impl PluginConfig {
    #[must_use]
    pub fn max_grid_dimension(&self) -> u32 {
        self.limits.layout.max_grid_dimension
    }

    #[must_use]
    pub fn max_panes_per_tab(&self) -> u32 {
        self.limits.layout.max_panes_per_tab
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawPluginConfig {
    version: u32,
    #[serde(default)]
    limits: LimitsConfig,
}

impl TryFrom<RawPluginConfig> for PluginConfig {
    type Error = PluginConfigError;

    fn try_from(raw: RawPluginConfig) -> Result<Self, Self::Error> {
        if raw.version != 1 {
            return Err(PluginConfigError::UnsupportedVersion(raw.version));
        }
        let value = raw.limits.layout.max_grid_dimension;
        if !(1..=MAX_CONFIGURABLE_GRID_DIMENSION).contains(&value) {
            return Err(PluginConfigError::InvalidMaxGridDimension {
                value,
                max: MAX_CONFIGURABLE_GRID_DIMENSION,
            });
        }
        let value = raw.limits.layout.max_panes_per_tab;
        if !(1..=MAX_CONFIGURABLE_PANES_PER_TAB).contains(&value) {
            return Err(PluginConfigError::InvalidMaxPanesPerTab {
                value,
                max: MAX_CONFIGURABLE_PANES_PER_TAB,
            });
        }
        Ok(Self {
            version: raw.version,
            limits: raw.limits,
        })
    }
}

impl Default for PluginConfig {
    fn default() -> Self {
        Self {
            version: 1,
            limits: LimitsConfig::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
struct LimitsConfig {
    layout: LayoutLimits,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
struct LayoutLimits {
    max_grid_dimension: u32,
    max_panes_per_tab: u32,
}

impl Default for LayoutLimits {
    fn default() -> Self {
        Self {
            max_grid_dimension: DEFAULT_MAX_GRID_DIMENSION,
            max_panes_per_tab: DEFAULT_MAX_PANES_PER_TAB,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PluginConfigError {
    #[error("failed to read plugin config file {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse plugin config: {0}")]
    Yaml(serde_yaml_ng::Error),
    #[error("failed to parse plugin config file {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_yaml_ng::Error,
    },
    #[error("unsupported plugin config version {0}; expected 1")]
    UnsupportedVersion(u32),
    #[error("limits.layout.maxGridDimension must be between 1 and {max}, got {value}")]
    InvalidMaxGridDimension { value: u32, max: u32 },
    #[error("limits.layout.maxPanesPerTab must be between 1 and {max}, got {value}")]
    InvalidMaxPanesPerTab { value: u32, max: u32 },
}

pub fn load_str(yaml: &str) -> Result<PluginConfig, PluginConfigError> {
    let raw: RawPluginConfig = serde_yaml_ng::from_str(yaml).map_err(PluginConfigError::Yaml)?;
    PluginConfig::try_from(raw)
}

pub fn load_from_dir(dir: impl AsRef<Path>) -> Result<PluginConfig, PluginConfigError> {
    let path = dir.as_ref().join(CONFIG_FILE_NAME);
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            let raw: RawPluginConfig =
                serde_yaml_ng::from_str(&text).map_err(|source| PluginConfigError::Parse {
                    path: path.clone(),
                    source,
                })?;
            PluginConfig::try_from(raw)
        }
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(PluginConfig::default()),
        Err(source) => Err(PluginConfigError::Io { path, source }),
    }
}

pub fn load_from_env() -> Result<PluginConfig, PluginConfigError> {
    let Some(dir) = std::env::var_os(CONFIG_DIR_ENV).filter(|value| !value.is_empty()) else {
        return Ok(PluginConfig::default());
    };
    load_from_dir(PathBuf::from(dir))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_documented_layout_limits() {
        assert_eq!(
            PluginConfig::default().max_grid_dimension(),
            DEFAULT_MAX_GRID_DIMENSION
        );
        assert_eq!(
            PluginConfig::default().max_panes_per_tab(),
            DEFAULT_MAX_PANES_PER_TAB
        );
    }

    #[test]
    fn example_plugin_config_loads() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/config.yaml");
        let config = load_from_dir(path.parent().expect("example has a parent"))
            .expect("example plugin config should load");
        assert_eq!(config.max_grid_dimension(), DEFAULT_MAX_GRID_DIMENSION);
        assert_eq!(config.max_panes_per_tab(), DEFAULT_MAX_PANES_PER_TAB);
    }

    #[test]
    fn missing_plugin_config_uses_defaults() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("does-not-exist");
        let config = load_from_dir(dir).expect("missing config should use defaults");
        assert_eq!(config, PluginConfig::default());
    }

    #[test]
    fn omitted_limit_uses_default() {
        let config = load_str("version: 1\n").expect("minimal config should load");
        assert_eq!(config.max_grid_dimension(), DEFAULT_MAX_GRID_DIMENSION);
        assert_eq!(config.max_panes_per_tab(), DEFAULT_MAX_PANES_PER_TAB);
    }

    #[test]
    fn configured_limit_is_loaded() {
        let config = load_str(
            "version: 1\nlimits:\n  layout:\n    maxGridDimension: 32\n    maxPanesPerTab: 16\n",
        )
        .expect("configured limit should load");
        assert_eq!(config.max_grid_dimension(), 32);
        assert_eq!(config.max_panes_per_tab(), 16);
    }

    #[test]
    fn unknown_key_is_rejected() {
        let err = load_str("version: 1\nunknown: true\n").expect_err("unknown key must fail");
        assert!(matches!(err, PluginConfigError::Yaml(_)));
    }

    #[test]
    fn duplicate_key_is_rejected() {
        let err = load_str("version: 1\nversion: 1\n").expect_err("duplicate key must fail");
        assert!(matches!(err, PluginConfigError::Yaml(_)));
    }

    #[test]
    fn unsupported_version_is_rejected() {
        let err = load_str("version: 2\n").expect_err("unsupported version must fail");
        assert!(matches!(err, PluginConfigError::UnsupportedVersion(2)));
    }

    #[test]
    fn zero_grid_limit_is_rejected() {
        let err = load_str("version: 1\nlimits:\n  layout:\n    maxGridDimension: 0\n")
            .expect_err("zero limit must fail");
        assert!(matches!(
            err,
            PluginConfigError::InvalidMaxGridDimension { value: 0, .. }
        ));
    }

    #[test]
    fn zero_pane_limit_is_rejected() {
        let err = load_str("version: 1\nlimits:\n  layout:\n    maxPanesPerTab: 0\n")
            .expect_err("zero pane limit must fail");
        assert!(matches!(
            err,
            PluginConfigError::InvalidMaxPanesPerTab { value: 0, .. }
        ));
    }

    #[test]
    fn grid_limit_above_safety_limit_is_rejected() {
        let yaml = format!(
            "version: 1\nlimits:\n  layout:\n    maxGridDimension: {}\n",
            MAX_CONFIGURABLE_GRID_DIMENSION + 1
        );
        let err = load_str(&yaml).expect_err("limit above safety maximum must fail");
        assert!(matches!(
            err,
            PluginConfigError::InvalidMaxGridDimension { .. }
        ));
    }

    #[test]
    fn pane_limit_above_safety_limit_is_rejected() {
        let yaml = format!(
            "version: 1\nlimits:\n  layout:\n    maxPanesPerTab: {}\n",
            MAX_CONFIGURABLE_PANES_PER_TAB + 1
        );
        let err = load_str(&yaml).expect_err("pane limit above safety maximum must fail");
        assert!(matches!(
            err,
            PluginConfigError::InvalidMaxPanesPerTab { .. }
        ));
    }

    #[test]
    fn direct_deserialization_cannot_bypass_grid_limit_validation() {
        let yaml = format!(
            "version: 1\nlimits:\n  layout:\n    maxGridDimension: {}\n",
            MAX_CONFIGURABLE_GRID_DIMENSION + 1
        );
        let err = serde_yaml_ng::from_str::<PluginConfig>(&yaml)
            .expect_err("direct deserialization must enforce the safety limit");
        assert!(err.to_string().contains("maxGridDimension"));
    }

    #[test]
    fn direct_deserialization_cannot_bypass_pane_limit_validation() {
        let yaml = format!(
            "version: 1\nlimits:\n  layout:\n    maxPanesPerTab: {}\n",
            MAX_CONFIGURABLE_PANES_PER_TAB + 1
        );
        let err = serde_yaml_ng::from_str::<PluginConfig>(&yaml)
            .expect_err("direct deserialization must enforce the pane safety limit");
        assert!(err.to_string().contains("maxPanesPerTab"));
    }
}
