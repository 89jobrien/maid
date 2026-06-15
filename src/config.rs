use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::MaidError;

#[derive(Deserialize, Default)]
struct ConvertConfig {
    #[serde(default)]
    pandoc: Vec<String>,
    #[serde(default)]
    marker: Vec<String>,
    #[serde(default)]
    mutool: Vec<String>,
    #[serde(default)]
    fallback: Option<String>,
}

#[derive(Deserialize)]
struct ConfigFile {
    #[serde(default)]
    directories: Vec<String>,
    #[serde(default)]
    categories: HashMap<String, Vec<String>>,
    #[serde(default)]
    destinations: HashMap<String, String>,
    #[serde(default)]
    convert: Option<ConvertConfig>,
    #[serde(default)]
    actions: HashMap<String, String>,
    #[serde(default)]
    stale: HashMap<String, u64>,
}

pub struct Config {
    pub directories: Vec<PathBuf>,
    lookup: HashMap<String, String>,
    destinations: HashMap<String, PathBuf>,
    convert_pandoc: Vec<String>,
    convert_marker: Vec<String>,
    convert_mutool: Vec<String>,
    convert_fallback: String,
    actions: HashMap<String, String>,
    stale: HashMap<String, u64>,
}

impl Config {
    pub fn load() -> Result<Self, MaidError> {
        let path = Self::config_path();
        if path.as_ref().is_some_and(|p| p.exists()) {
            let contents = std::fs::read_to_string(path.unwrap())?;
            let file: ConfigFile =
                toml::from_str(&contents).map_err(|e| MaidError::ConfigError(e.to_string()))?;

            let directories = if file.directories.is_empty() {
                Self::default_directories()
            } else {
                file.directories.iter().map(|d| expand_path(d)).collect()
            };

            let categories = if file.categories.is_empty() {
                Self::default_categories()
            } else {
                file.categories
            };

            let destinations: HashMap<String, PathBuf> = file
                .destinations
                .iter()
                .map(|(k, v)| (k.clone(), expand_path(v)))
                .collect();

            let convert = file.convert.unwrap_or_default();

            Ok(Self::build(
                directories,
                categories,
                destinations,
                convert.pandoc,
                convert.marker,
                convert.mutool,
                convert.fallback.unwrap_or_else(|| "pandoc".into()),
                file.actions,
                file.stale,
            ))
        } else {
            Ok(Self::defaults())
        }
    }

    pub fn defaults() -> Self {
        Self::build(
            Self::default_directories(),
            Self::default_categories(),
            HashMap::new(),
            vec![],
            vec![],
            vec![],
            "pandoc".into(),
            HashMap::new(),
            HashMap::new(),
        )
    }

    pub fn classify(&self, ext: &str) -> &str {
        self.lookup
            .get(&ext.to_lowercase())
            .map(|s| s.as_str())
            .unwrap_or("unknown")
    }

    pub fn destination(&self, category: &str, source_dir: &Path) -> PathBuf {
        if let Some(dest) = self.destinations.get(category) {
            dest.clone()
        } else {
            source_dir.join(category)
        }
    }

    pub fn quarantine_dir(&self) -> PathBuf {
        self.destinations
            .get("quarantine")
            .cloned()
            .unwrap_or_else(|| {
                dirs::data_dir()
                    .unwrap_or_else(|| PathBuf::from("/tmp"))
                    .join("maid")
                    .join("quarantine")
            })
    }

    pub fn archive_dir(&self) -> PathBuf {
        self.destinations
            .get("archive")
            .cloned()
            .unwrap_or_else(|| {
                dirs::home_dir()
                    .unwrap_or_else(|| PathBuf::from("/tmp"))
                    .join("Documents")
                    .join("_RepoArchive")
                    .join("maid")
            })
    }

    /// Returns the action for a category: "move" (default), "note", etc.
    pub fn action_for(&self, category: &str) -> &str {
        self.actions
            .get(category)
            .map(|s| s.as_str())
            .unwrap_or("move")
    }

    /// Returns the stale threshold in days for a category, or None.
    pub fn stale_days(&self, category: &str) -> Option<u64> {
        self.stale.get(category).copied()
    }

    pub fn converter_for(&self, ext: &str) -> Option<&str> {
        let ext_lower = ext.to_lowercase();
        if self.convert_marker.iter().any(|e| e == &ext_lower) {
            if which_exists("marker_single") {
                return Some("marker");
            }
            let fallback = self.convert_fallback.as_str();
            if !fallback.is_empty() {
                return Some(fallback);
            }
            return None;
        }
        if self.convert_mutool.iter().any(|e| e == &ext_lower) {
            return Some("mutool");
        }
        if self.convert_pandoc.iter().any(|e| e == &ext_lower) {
            return Some("pandoc");
        }
        None
    }

    #[allow(clippy::too_many_arguments)]
    fn build(
        directories: Vec<PathBuf>,
        categories: HashMap<String, Vec<String>>,
        destinations: HashMap<String, PathBuf>,
        convert_pandoc: Vec<String>,
        convert_marker: Vec<String>,
        convert_mutool: Vec<String>,
        convert_fallback: String,
        actions: HashMap<String, String>,
        stale: HashMap<String, u64>,
    ) -> Self {
        let mut lookup = HashMap::new();
        for (folder, exts) in &categories {
            for ext in exts {
                lookup.insert(ext.to_lowercase(), folder.clone());
            }
        }
        Self {
            directories,
            lookup,
            destinations,
            convert_pandoc,
            convert_marker,
            convert_mutool,
            convert_fallback,
            actions,
            stale,
        }
    }

    fn default_directories() -> Vec<PathBuf> {
        dirs::home_dir()
            .map(|home| {
                vec![
                    home.join("Documents"),
                    home.join("Downloads"),
                    home.join("Desktop"),
                ]
            })
            .unwrap_or_default()
    }

    fn default_categories() -> HashMap<String, Vec<String>> {
        HashMap::from([
            (
                "images".into(),
                vec!["jpg", "jpeg", "png", "gif", "svg", "webp"]
                    .into_iter()
                    .map(String::from)
                    .collect(),
            ),
            (
                "documents".into(),
                vec!["pdf", "docx", "doc", "txt", "xlsx", "pptx"]
                    .into_iter()
                    .map(String::from)
                    .collect(),
            ),
            (
                "video".into(),
                vec!["mp4", "mov", "avi", "mkv"]
                    .into_iter()
                    .map(String::from)
                    .collect(),
            ),
            (
                "audio".into(),
                vec!["mp3", "wav", "flac", "aac"]
                    .into_iter()
                    .map(String::from)
                    .collect(),
            ),
            (
                "code".into(),
                vec!["rs", "py", "js", "ts", "html", "css", "json"]
                    .into_iter()
                    .map(String::from)
                    .collect(),
            ),
            (
                "archives".into(),
                vec!["zip", "tar", "gz", "rar"]
                    .into_iter()
                    .map(String::from)
                    .collect(),
            ),
        ])
    }

    fn config_path() -> Option<PathBuf> {
        let xdg = dirs::home_dir().map(|h| h.join(".config").join("maid").join("config.toml"));
        if xdg.as_ref().is_some_and(|p| p.exists()) {
            return xdg;
        }
        dirs::config_dir().map(|d| d.join("maid").join("config.toml"))
    }
}

pub fn expand_path(path: &str) -> PathBuf {
    if path.starts_with("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(&path[2..]);
        }
    }
    PathBuf::from(path)
}

fn which_exists(cmd: &str) -> bool {
    std::process::Command::new("which")
        .arg(cmd)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}
