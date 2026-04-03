use std::collections::HashSet;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const CONFIG_DIRNAME: &str = "adb-ui-rs";
const CONFIG_FILENAME: &str = "adb-ui-rs.json";

const DEFAULT_TAGS: &[&str] = &["SDL", "SDL/APP", "GameActivity", "NativeCrashReporter"];
const DEFAULT_BUNDLE_ID: &str = "com.appname.app";
const DEFAULT_ACTIVITY: &str = "";

#[derive(Debug, Clone)]
pub struct LoadedConfig {
    pub config: AppConfig,
    pub path: PathBuf,
    pub warnings: Vec<String>,
}

/// A local-to-remote directory mapping for game data deploy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployDir {
    /// Human-readable label (e.g. "Game Paks").
    pub label: String,
    /// Local directory path (absolute or relative to CWD).
    pub local_path: String,
    /// Remote suffix under the app's files directory (e.g. "pack").
    pub remote_suffix: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// Android package / bundle ID.
    pub bundle_id: String,
    /// Logcat tag filters.
    pub logcat_tags: Vec<String>,
    /// Fully qualified activity class for `am start -n` launch.
    /// If empty, falls back to monkey launcher.
    #[serde(default)]
    pub activity_class: String,
    /// Configurable directories to deploy to device.
    #[serde(default)]
    pub deploy_dirs: Vec<DeployDir>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            bundle_id: DEFAULT_BUNDLE_ID.to_string(),
            logcat_tags: DEFAULT_TAGS.iter().map(|tag| (*tag).to_string()).collect(),
            activity_class: DEFAULT_ACTIVITY.to_string(),
            deploy_dirs: Vec::new(),
        }
    }
}

impl AppConfig {
    pub fn path() -> PathBuf {
        default_config_path()
    }

    pub fn legacy_path() -> PathBuf {
        legacy_config_path()
    }

    pub fn load() -> LoadedConfig {
        let path = Self::path();
        let mut warnings = Vec::new();

        match load_from_path(&path) {
            Ok(Some(config)) => {
                return LoadedConfig {
                    config: config.normalized(),
                    path,
                    warnings,
                };
            }
            Ok(None) => {}
            Err(error) => warnings.push(format!(
                "Failed to load config from {}: {error}",
                path.display()
            )),
        }

        let legacy_path = Self::legacy_path();
        if legacy_path != path {
            match load_from_path(&legacy_path) {
                Ok(Some(config)) => {
                    warnings.push(format!(
                        "Loaded legacy config from {}. Future saves use {}.",
                        legacy_path.display(),
                        path.display()
                    ));
                    return LoadedConfig {
                        config: config.normalized(),
                        path,
                        warnings,
                    };
                }
                Ok(None) => {}
                Err(error) => warnings.push(format!(
                    "Failed to load legacy config from {}: {error}",
                    legacy_path.display()
                )),
            }
        }

        LoadedConfig {
            config: Self::default(),
            path,
            warnings,
        }
    }

    pub fn save(&self) -> Result<(), String> {
        let path = Self::path();
        let config = self.normalized();
        let json = serde_json::to_string_pretty(&config)
            .map_err(|error| format!("Serialize failed: {error}"))?;

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "Failed to create config directory {}: {error}",
                    parent.display()
                )
            })?;
        }

        let temp_path = path.with_extension("json.tmp");
        fs::write(&temp_path, json)
            .map_err(|error| format!("Failed to write {}: {error}", temp_path.display()))?;

        if let Err(rename_error) = fs::rename(&temp_path, &path) {
            if path.exists() {
                fs::remove_file(&path)
                    .map_err(|error| format!("Failed to replace {}: {error}", path.display()))?;
                fs::rename(&temp_path, &path)
                    .map_err(|error| format!("Failed to finalize config save: {error}"))?;
            } else {
                let _ = fs::remove_file(&temp_path);
                return Err(format!(
                    "Failed to move config into place ({} -> {}): {rename_error}",
                    temp_path.display(),
                    path.display()
                ));
            }
        }

        Ok(())
    }

    /// Build logcat filter arguments.
    /// Empty tag lists mean "show all logcat output".
    pub fn logcat_filter_args(&self) -> Vec<String> {
        let tags: Vec<String> = self
            .logcat_tags
            .iter()
            .map(|tag| tag.trim())
            .filter(|tag| !tag.is_empty())
            .map(|tag| format!("{tag}:V"))
            .collect();

        if tags.is_empty() {
            Vec::new()
        } else {
            let mut args = Vec::with_capacity(tags.len() + 1);
            args.push("*:S".to_string());
            args.extend(tags);
            args
        }
    }

    fn normalized(&self) -> Self {
        let mut seen_tags = HashSet::new();
        let logcat_tags = self
            .logcat_tags
            .iter()
            .map(|tag| tag.trim())
            .filter(|tag| !tag.is_empty())
            .filter_map(|tag| {
                let owned = tag.to_string();
                if seen_tags.insert(owned.clone()) {
                    Some(owned)
                } else {
                    None
                }
            })
            .collect();

        let deploy_dirs = self
            .deploy_dirs
            .iter()
            .map(|dir| DeployDir {
                label: dir.label.trim().to_string(),
                local_path: dir.local_path.trim().to_string(),
                remote_suffix: dir.remote_suffix.trim().to_string(),
            })
            .collect();

        let bundle_id = self.bundle_id.trim();
        Self {
            bundle_id: if bundle_id.is_empty() {
                DEFAULT_BUNDLE_ID.to_string()
            } else {
                bundle_id.to_string()
            },
            logcat_tags,
            activity_class: self.activity_class.trim().to_string(),
            deploy_dirs,
        }
    }
}

fn load_from_path(path: &Path) -> Result<Option<AppConfig>, String> {
    match fs::read_to_string(path) {
        Ok(json) => serde_json::from_str(&json)
            .map(Some)
            .map_err(|error| format!("Invalid JSON: {error}")),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

fn default_config_path() -> PathBuf {
    config_root_dir().join(CONFIG_FILENAME)
}

fn legacy_config_path() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            return dir.join(CONFIG_FILENAME);
        }
    }
    PathBuf::from(CONFIG_FILENAME)
}

fn config_root_dir() -> PathBuf {
    if let Some(dir) = platform_config_dir() {
        return dir.join(CONFIG_DIRNAME);
    }
    PathBuf::from(CONFIG_DIRNAME)
}

fn platform_config_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .or_else(|| home_dir().map(|path| path.join("AppData/Roaming")))
    }

    #[cfg(target_os = "macos")]
    {
        home_dir().map(|path| path.join("Library/Application Support"))
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| home_dir().map(|path| path.join(".config")))
    }

    #[cfg(not(any(windows, unix)))]
    {
        None
    }
}

fn home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("USERPROFILE")
            .map(PathBuf::from)
            .or_else(|| {
                let drive = std::env::var_os("HOMEDRIVE")?;
                let path = std::env::var_os("HOMEPATH")?;
                Some(PathBuf::from(format!(
                    "{}{}",
                    PathBuf::from(drive).display(),
                    PathBuf::from(path).display()
                )))
            })
    }

    #[cfg(not(windows))]
    {
        std::env::var_os("HOME").map(PathBuf::from)
    }
}

#[cfg(test)]
mod tests {
    use super::{AppConfig, DeployDir};

    #[test]
    fn logcat_filter_args_is_empty_when_no_tags_exist() {
        let config = AppConfig {
            logcat_tags: vec![String::new(), "   ".into()],
            ..AppConfig::default()
        };

        assert!(config.logcat_filter_args().is_empty());
    }

    #[test]
    fn normalized_config_trims_and_deduplicates_tags() {
        let config = AppConfig {
            bundle_id: "  com.example.app  ".into(),
            logcat_tags: vec![
                " SDL ".into(),
                "SDL".into(),
                String::new(),
                "NativeCrashReporter".into(),
            ],
            activity_class: "  .MainActivity ".into(),
            deploy_dirs: vec![DeployDir {
                label: "  Game Paks ".into(),
                local_path: "  C:/game/paks  ".into(),
                remote_suffix: "  pack  ".into(),
            }],
        };

        let normalized = config.normalized();

        assert_eq!(normalized.bundle_id, "com.example.app");
        assert_eq!(
            normalized.logcat_tags,
            vec!["SDL".to_string(), "NativeCrashReporter".to_string()]
        );
        assert_eq!(normalized.activity_class, ".MainActivity");
        assert_eq!(normalized.deploy_dirs[0].label, "Game Paks");
        assert_eq!(normalized.deploy_dirs[0].local_path, "C:/game/paks");
        assert_eq!(normalized.deploy_dirs[0].remote_suffix, "pack");
    }
}
