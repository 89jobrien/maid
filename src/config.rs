//! Loads Maid's file categories, destinations, actions, and conversion settings.

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
    /// Loads the user configuration, falling back to built-in defaults when absent.
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

    /// Builds the default configuration for common home-directory folders and file types.
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

    /// Returns the configured category for a file extension, or `"unknown"`.
    pub fn classify(&self, ext: &str) -> &str {
        self.lookup
            .get(&ext.to_lowercase())
            .map(|s| s.as_str())
            .unwrap_or("unknown")
    }

    /// Resolves a category's configured destination or a subdirectory of the source.
    pub fn destination(&self, category: &str, source_dir: &Path) -> PathBuf {
        if let Some(dest) = self.destinations.get(category) {
            dest.clone()
        } else {
            source_dir.join(category)
        }
    }

    /// Returns the configured quarantine directory or Maid's data-directory fallback.
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

    /// Returns the configured archive directory or the default repository archive path.
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

    /// Selects the configured converter for an extension, honoring Marker fallback settings.
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

/// Expands a leading `~/` against the user's home directory when available.
pub fn expand_path(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest);
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

/// Builds a config with caller-supplied categories and destinations.
/// Test-only: lets `organiser` tests exercise routing without touching
/// the user's real `config.toml`.
#[cfg(test)]
pub(crate) fn test_config_with(
    categories: HashMap<String, Vec<String>>,
    destinations: HashMap<String, PathBuf>,
) -> Config {
    Config::build(
        vec![],
        categories,
        destinations,
        vec![],
        vec![],
        vec![],
        "pandoc".to_string(),
        HashMap::new(),
        HashMap::new(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a config whose classification table is exactly `pairs`.
    fn with_categories(pairs: &[(&str, &[&str])]) -> Config {
        let mut c = Config::defaults();
        c.lookup = pairs
            .iter()
            .flat_map(|(folder, exts)| {
                exts.iter()
                    .map(move |e| (e.to_lowercase(), (*folder).to_string()))
            })
            .collect();
        c
    }

    #[test]
    fn classify_maps_extension_to_folder() {
        let c = with_categories(&[("images", &["jpg", "png"])]);
        assert_eq!(c.classify("jpg"), "images");
        assert_eq!(c.classify("png"), "images");
    }

    #[test]
    fn classify_is_case_insensitive() {
        let c = with_categories(&[("images", &["jpg"])]);
        assert_eq!(c.classify("JPG"), "images");
        assert_eq!(c.classify("JpG"), "images");
    }

    #[test]
    fn classify_falls_back_to_unknown() {
        let c = with_categories(&[("images", &["jpg"])]);
        assert_eq!(c.classify("xyz"), "unknown");
        assert_eq!(c.classify(""), "unknown");
    }

    #[test]
    fn default_categories_cover_documented_folders() {
        let c = Config::defaults();
        assert_eq!(c.classify("jpg"), "images");
        assert_eq!(c.classify("pdf"), "documents");
        assert_eq!(c.classify("mp4"), "video");
        assert_eq!(c.classify("mp3"), "audio");
        assert_eq!(c.classify("rs"), "code");
        assert_eq!(c.classify("zip"), "archives");
    }

    #[test]
    fn markdown_is_not_a_default_category() {
        let c = Config::defaults();
        assert_eq!(c.classify("md"), "unknown");
        assert_eq!(c.classify("mdx"), "unknown");
    }

    #[test]
    fn destination_defaults_to_subdirectory_of_source() {
        let c = Config::defaults();
        assert_eq!(
            c.destination("images", Path::new("/tmp/dl")),
            PathBuf::from("/tmp/dl/images")
        );
    }

    #[test]
    fn destination_prefers_configured_path() {
        let mut c = Config::defaults();
        c.destinations
            .insert("images".into(), PathBuf::from("/vault/img"));
        assert_eq!(
            c.destination("images", Path::new("/tmp/dl")),
            PathBuf::from("/vault/img")
        );
    }

    #[test]
    fn action_defaults_to_move() {
        let c = Config::defaults();
        assert_eq!(c.action_for("images"), "move");
        assert_eq!(c.action_for("unconfigured"), "move");
    }

    #[test]
    fn action_reads_configured_value() {
        let mut c = Config::defaults();
        c.actions.insert("diagnostics".into(), "note".into());
        assert_eq!(c.action_for("diagnostics"), "note");
    }

    #[test]
    fn stale_days_absent_by_default() {
        assert_eq!(Config::defaults().stale_days("images"), None);
    }

    #[test]
    fn stale_days_reads_configured_value() {
        let mut c = Config::defaults();
        c.stale.insert("diagnostics".into(), 90);
        assert_eq!(c.stale_days("diagnostics"), Some(90));
    }

    #[test]
    fn no_conversion_configured_by_default() {
        let c = Config::defaults();
        assert_eq!(c.converter_for("pdf"), None);
        assert_eq!(c.converter_for("docx"), None);
    }

    #[test]
    fn converter_selects_mutool_then_pandoc() {
        let mut c = Config::defaults();
        c.convert_mutool = vec!["pdf".into()];
        c.convert_pandoc = vec!["docx".into()];
        assert_eq!(c.converter_for("pdf"), Some("mutool"));
        assert_eq!(c.converter_for("docx"), Some("pandoc"));
        assert_eq!(c.converter_for("jpg"), None);
    }

    #[test]
    fn converter_matching_is_case_insensitive() {
        let mut c = Config::defaults();
        c.convert_pandoc = vec!["docx".into()];
        assert_eq!(c.converter_for("DOCX"), Some("pandoc"));
    }

    #[test]
    fn converter_marker_resolves_to_marker_or_fallback() {
        let mut c = Config::defaults();
        c.convert_marker = vec!["pdf".into()];
        c.convert_fallback = "pandoc".into();
        assert!(matches!(
            c.converter_for("pdf"),
            Some("marker") | Some("pandoc")
        ));
    }

    #[test]
    fn converter_marker_without_fallback_or_binary_is_none() {
        let mut c = Config::defaults();
        c.convert_marker = vec!["pdf".into()];
        c.convert_fallback = String::new();
        if !which_exists("marker_single") {
            assert_eq!(c.converter_for("pdf"), None);
        }
    }

    #[test]
    fn expand_path_expands_home_prefix() {
        let Some(home) = dirs::home_dir() else {
            return;
        };
        assert_eq!(expand_path("~/Downloads"), home.join("Downloads"));
    }

    #[test]
    fn expand_path_leaves_other_paths_alone() {
        assert_eq!(expand_path("/abs/path"), PathBuf::from("/abs/path"));
        assert_eq!(expand_path("rel/path"), PathBuf::from("rel/path"));
        assert_eq!(expand_path("~"), PathBuf::from("~"));
    }

    #[test]
    fn quarantine_and_archive_have_fallbacks() {
        let c = Config::defaults();
        assert!(c.quarantine_dir().to_string_lossy().contains("quarantine"));
        assert!(c.archive_dir().to_string_lossy().contains("_RepoArchive"));
    }

    #[test]
    fn quarantine_and_archive_honour_config() {
        let mut c = Config::defaults();
        c.destinations
            .insert("quarantine".into(), PathBuf::from("/q"));
        c.destinations.insert("archive".into(), PathBuf::from("/a"));
        assert_eq!(c.quarantine_dir(), PathBuf::from("/q"));
        assert_eq!(c.archive_dir(), PathBuf::from("/a"));
    }
}
