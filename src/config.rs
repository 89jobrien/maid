//! Loads Maid's file categories, destinations, actions, and conversion settings.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::MaidError;

const PROJECT_CONFIG_FILE: &str = "maid.toml";

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

#[derive(Deserialize, Default)]
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
    #[serde(default)]
    routes: Vec<Route>,
}

/// Where a routed file's `{date}` placeholder is read from.
#[derive(Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub enum DateSource {
    /// Read the date from the file's modification time.
    #[default]
    Mtime,
    /// Parse a date embedded in the filename stem.
    Stem,
}

/// A filename-pattern route that overrides the extension-based destination.
#[derive(Deserialize)]
pub struct Route {
    /// Human-readable label, shown in preview output.
    pub name: String,
    /// Wildcard patterns (`*` only) matched case-insensitively against the filename.
    #[serde(default)]
    pub patterns: Vec<String>,
    /// Destination directory. A leading `~/` is expanded at load.
    pub destination: String,
    /// Optional filename template. Supports `{stem}`, `{date}`, and `{ext}`.
    #[serde(default, rename = "rename")]
    pub rename: Option<String>,
    #[serde(default)]
    pub date_source: DateSource,
}

/// A route with its destination expanded and ready to match against.
#[derive(Clone)]
pub struct ResolvedRoute {
    name: String,
    patterns: Vec<String>,
    destination: PathBuf,
    rename: Option<String>,
    date_source: DateSource,
}

impl ResolvedRoute {
    /// The route's display name, surfaced in preview output.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The directory a matched file is routed into.
    pub fn destination(&self) -> &Path {
        &self.destination
    }

    /// Renders the configured rename template, falling back to `fallback_name`
    /// when no template is set or the date cannot be resolved.
    pub fn target_name(&self, stem: &str, ext: &str, source: &Path, fallback_name: &str) -> String {
        let Some(template) = &self.rename else {
            return fallback_name.to_string();
        };

        let date = match self.date_source {
            DateSource::Mtime => modified_date(source),
            DateSource::Stem => stem_date(stem),
        };

        let Some(date) = date else {
            return fallback_name.to_string();
        };

        template
            .replace("{stem}", stem)
            .replace("{date}", &date)
            .replace("{ext}", ext)
    }
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
    routes: Vec<ResolvedRoute>,
}

impl Config {
    /// Loads the user configuration, falling back to built-in defaults when absent.
    pub fn load() -> Result<Self, MaidError> {
        Ok(Self::from_file(Self::global_file()?))
    }

    /// Loads the configuration that applies to `target`: the global config, with the
    /// nearest `maid.toml` at or above `target` layered over it when one exists.
    pub fn load_for(target: &Path) -> Result<Self, MaidError> {
        let mut file = Self::global_file()?;
        if let Some(path) = project_config_for(target) {
            let project = Self::read_file(&path)?;
            let dir = path.parent().unwrap_or(Path::new("."));
            file = overlay(file, project, dir);
        }
        Ok(Self::from_file(file))
    }

    fn global_file() -> Result<ConfigFile, MaidError> {
        match Self::config_path() {
            Some(path) if path.exists() => Self::read_file(&path),
            _ => Ok(ConfigFile::default()),
        }
    }

    fn read_file(path: &Path) -> Result<ConfigFile, MaidError> {
        let contents = std::fs::read_to_string(path)?;
        toml::from_str(&contents)
            .map_err(|e| MaidError::ConfigError(format!("{}: {}", path.display(), e)))
    }

    fn from_file(file: ConfigFile) -> Self {
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

        Self::build(
            directories,
            categories,
            destinations,
            convert.pandoc,
            convert.marker,
            convert.mutool,
            convert.fallback.unwrap_or_else(|| "pandoc".into()),
            file.actions,
            file.stale,
            file.routes,
        )
    }

    /// Builds the default configuration for common home-directory folders and file types.
    #[cfg(test)]
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
            vec![],
        )
    }

    /// Returns the first route whose patterns match `filename`.
    ///
    /// Routes are evaluated in config order, so a specific pattern placed above a
    /// broad one wins. Extension-based classification is untouched: a route only
    /// overrides *where* a file lands, never what Maid thinks it is.
    pub fn route_for(&self, filename: &str) -> Option<&ResolvedRoute> {
        self.routes
            .iter()
            .find(|r| r.patterns.iter().any(|p| wildcard_match(p, filename)))
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
        routes: Vec<Route>,
    ) -> Self {
        let mut lookup = HashMap::new();
        for (folder, exts) in &categories {
            for ext in exts {
                lookup.insert(ext.to_lowercase(), folder.clone());
            }
        }
        let routes = routes
            .into_iter()
            .map(|r| ResolvedRoute {
                name: r.name,
                patterns: r.patterns,
                destination: expand_path(&r.destination),
                rename: r.rename,
                date_source: r.date_source,
            })
            .collect();
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
            routes,
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

/// Returns the nearest `maid.toml` at or above `target`, if one exists.
pub fn project_config_for(target: &Path) -> Option<PathBuf> {
    let start = std::fs::canonicalize(target).unwrap_or_else(|_| target.to_path_buf());
    start
        .ancestors()
        .map(|dir| dir.join(PROJECT_CONFIG_FILE))
        .find(|p| p.is_file())
}

/// Layers a project `maid.toml` over the global file.
///
/// Categories and `[convert]` are replaced when the project sets them.
/// Destinations, actions and stale merge per key. Project routes run before
/// global ones, so a vault can claim its files first. `directories` stays
/// global because it only matters when maid runs with no path. Relative
/// paths in the project file resolve against `dir`, the file's own directory.
fn overlay(mut base: ConfigFile, project: ConfigFile, dir: &Path) -> ConfigFile {
    if !project.categories.is_empty() {
        base.categories = project.categories;
    }
    base.destinations.extend(
        project
            .destinations
            .into_iter()
            .map(|(k, v)| (k, anchor(&v, dir))),
    );
    if project.convert.is_some() {
        base.convert = project.convert;
    }
    base.actions.extend(project.actions);
    base.stale.extend(project.stale);

    let mut routes: Vec<Route> = project
        .routes
        .into_iter()
        .map(|mut r| {
            r.destination = anchor(&r.destination, dir);
            r
        })
        .collect();
    routes.append(&mut base.routes);
    base.routes = routes;
    base
}

/// Resolves a relative path against `dir`. Absolute and `~` paths pass through.
fn anchor(path: &str, dir: &Path) -> String {
    if path.starts_with('/') || path == "~" || path.starts_with("~/") {
        path.to_string()
    } else {
        dir.join(path).to_string_lossy().into_owned()
    }
}

fn which_exists(cmd: &str) -> bool {
    std::process::Command::new("which")
        .arg(cmd)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Matches `text` against a `*`-only wildcard `pattern`, case-insensitively.
///
/// Deliberately not a full glob: patterns come from the user's own config, and
/// supporting only `*` keeps matching predictable and dependency-free. Multiple
/// `*`s are allowed and matched left to right.
fn wildcard_match(pattern: &str, text: &str) -> bool {
    let pattern = pattern.to_lowercase();
    let text = text.to_lowercase();

    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == text;
    }

    let (first, middles) = parts.split_first().expect("pattern has at least one part");
    let (last, middles) = middles
        .split_last()
        .expect("pattern has at least two parts");

    let Some(rest) = text.strip_prefix(*first) else {
        return false;
    };
    let mut rest = rest;

    for mid in middles {
        let Some(idx) = rest.find(*mid) else {
            return false;
        };
        rest = &rest[idx + mid.len()..];
    }

    rest.ends_with(last)
}

/// Formats a path's modification time as `YYYY-MM-DD`, or None if unreadable.
fn modified_date(path: &Path) -> Option<String> {
    let mtime = path.metadata().ok()?.modified().ok()?;
    let datetime: chrono::DateTime<chrono::Local> = mtime.into();
    Some(datetime.format("%Y-%m-%d").to_string())
}

/// Extracts a `YYYYMMDD` or `YYYY-MM-DD` date embedded in a filename stem and
/// normalises it to `YYYY-MM-DD`. Returns None when no valid date is present.
fn stem_date(stem: &str) -> Option<String> {
    use chrono::NaiveDate;

    let digits: Vec<char> = stem.chars().filter(char::is_ascii_digit).collect();
    for window in digits.windows(8) {
        let run: String = window.iter().collect();
        if let Ok(date) = NaiveDate::parse_from_str(&run, "%Y%m%d") {
            return Some(date.format("%Y-%m-%d").to_string());
        }
    }

    let chars: Vec<char> = stem.chars().collect();
    for window in chars.windows(10) {
        let run: String = window.iter().collect();
        if let Ok(date) = NaiveDate::parse_from_str(&run, "%Y-%m-%d") {
            return Some(date.format("%Y-%m-%d").to_string());
        }
    }

    None
}

/// Builds a config with caller-supplied categories and destinations.
/// Test-only: lets `organiser` tests exercise routing without touching
/// the user's real `config.toml`.
#[cfg(test)]
pub(crate) fn test_config_with(
    categories: HashMap<String, Vec<String>>,
    destinations: HashMap<String, PathBuf>,
) -> Config {
    test_config_with_routes(categories, destinations, vec![])
}

/// Same as [`test_config_with`] but lets `organiser` tests exercise routes.
#[cfg(test)]
pub(crate) fn test_config_with_routes(
    categories: HashMap<String, Vec<String>>,
    destinations: HashMap<String, PathBuf>,
    routes: Vec<Route>,
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
        routes,
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

    // --- Routes ---

    fn route(name: &str, patterns: &[&str], destination: &str) -> Route {
        Route {
            name: name.into(),
            patterns: patterns.iter().map(|p| (*p).to_string()).collect(),
            destination: destination.into(),
            rename: None,
            date_source: DateSource::Mtime,
        }
    }

    fn config_with_routes(routes: Vec<Route>) -> Config {
        test_config_with_routes(HashMap::new(), HashMap::new(), routes)
    }

    #[test]
    fn wildcard_exact_without_star() {
        assert!(wildcard_match("notes.md", "notes.md"));
        assert!(!wildcard_match("notes.md", "notes.txt"));
    }

    #[test]
    fn wildcard_prefix_suffix_and_infix() {
        assert!(wildcard_match(
            "ClaudeCodeInsights-*",
            "ClaudeCodeInsights-20260330.pdf"
        ));
        assert!(wildcard_match(
            "*persona*",
            "pieces_professional_persona1.pdf"
        ));
        assert!(wildcard_match("*.pdf", "report.pdf"));
    }

    #[test]
    fn wildcard_is_case_insensitive() {
        assert!(wildcard_match(
            "claudecodeinsights-*",
            "ClaudeCodeInsights-2026.pdf"
        ));
        assert!(wildcard_match("*PERSONA*", "pieces_persona.pdf"));
    }

    #[test]
    fn wildcard_non_match_returns_false() {
        assert!(!wildcard_match("ClaudeCodeInsights-*", "report.pdf"));
        assert!(!wildcard_match("*persona*", "hud.ai-report.pdf"));
        assert!(!wildcard_match("a*b", "ac"));
    }

    #[test]
    fn wildcard_supports_multiple_stars() {
        assert!(wildcard_match(
            "*insights*pdf",
            "ClaudeCodeInsights-2026.pdf"
        ));
        assert!(!wildcard_match(
            "*insights*png",
            "ClaudeCodeInsights-2026.pdf"
        ));
    }

    #[test]
    fn stem_date_normalises_compact_and_dashed() {
        assert_eq!(
            stem_date("ClaudeCodeInsights-20260330").as_deref(),
            Some("2026-03-30")
        );
        assert_eq!(
            stem_date("persona-2026-04-02").as_deref(),
            Some("2026-04-02")
        );
    }

    #[test]
    fn stem_date_is_none_without_a_valid_date() {
        assert_eq!(stem_date("pieces_professional_persona1"), None);
        assert_eq!(stem_date("report-99999999"), None);
        assert_eq!(stem_date("short123"), None);
    }

    #[test]
    fn route_for_returns_first_matching_route() {
        let c = config_with_routes(vec![
            route("insights", &["ClaudeCodeInsights-*"], "/dest/a"),
            route("persona", &["*persona*"], "/dest/b"),
        ]);
        assert_eq!(
            c.route_for("ClaudeCodeInsights-2026.pdf").unwrap().name(),
            "insights"
        );
        assert_eq!(c.route_for("x_persona.pdf").unwrap().name(), "persona");
        assert!(c.route_for("unrelated.pdf").is_none());
    }

    // --- Project maid.toml ---

    fn temp_tree(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("maid-config-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("vault/a/b")).expect("create tree");
        root
    }

    #[test]
    fn project_config_is_found_in_an_ancestor() {
        let root = temp_tree("discover");
        let vault = root.join("vault");
        std::fs::write(vault.join("maid.toml"), "").expect("write");

        assert_eq!(
            project_config_for(&vault.join("a/b")),
            Some(
                std::fs::canonicalize(&vault)
                    .expect("canon")
                    .join("maid.toml")
            )
        );
        std::fs::remove_dir_all(&root).expect("clean up");
    }

    #[test]
    fn project_config_is_none_without_a_maid_toml() {
        let root = temp_tree("discover-none");
        assert!(project_config_for(&root.join("vault/a")).is_none());
        std::fs::remove_dir_all(&root).expect("clean up");
    }

    #[test]
    fn overlay_project_destination_wins_and_anchors_to_its_dir() {
        let global: ConfigFile = toml::from_str(
            r#"
            [destinations]
            images = "/global/images"
            notes = "/global/notes"
            "#,
        )
        .expect("global");
        let project: ConfigFile = toml::from_str(
            r#"
            [destinations]
            images = "img"
            "#,
        )
        .expect("project");

        let merged = overlay(global, project, Path::new("/vault"));
        assert_eq!(merged.destinations["images"], "/vault/img");
        assert_eq!(merged.destinations["notes"], "/global/notes");
    }

    #[test]
    fn overlay_keeps_global_categories_unless_project_sets_them() {
        let global: ConfigFile = toml::from_str(
            r#"
            [categories]
            images = ["jpg"]
            "#,
        )
        .expect("global");
        let project: ConfigFile =
            toml::from_str("[actions]\ndiagnostics = \"note\"\n").expect("project");

        let merged = overlay(global, project, Path::new("/vault"));
        assert_eq!(merged.categories["images"], vec!["jpg".to_string()]);
        assert_eq!(merged.actions["diagnostics"], "note");
    }

    #[test]
    fn overlay_puts_project_routes_first_and_anchors_them() {
        let global: ConfigFile = toml::from_str(
            r#"
            [[routes]]
            name = "global"
            patterns = ["*.pdf"]
            destination = "/global/pdf"
            "#,
        )
        .expect("global");
        let project: ConfigFile = toml::from_str(
            r#"
            [[routes]]
            name = "vault"
            patterns = ["*.pdf"]
            destination = "refs"
            "#,
        )
        .expect("project");

        let merged = overlay(global, project, Path::new("/vault"));
        let names: Vec<&str> = merged.routes.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["vault", "global"]);
        assert_eq!(merged.routes[0].destination, "/vault/refs");
    }

    #[test]
    fn anchor_leaves_absolute_and_home_paths_alone() {
        assert_eq!(anchor("/abs", Path::new("/vault")), "/abs");
        assert_eq!(anchor("~/x", Path::new("/vault")), "~/x");
        assert_eq!(anchor("rel", Path::new("/vault")), "/vault/rel");
    }

    #[test]
    fn no_routes_configured_means_no_match() {
        assert!(Config::defaults().route_for("anything.pdf").is_none());
    }

    #[test]
    fn route_destination_expands_home_prefix() {
        let Some(home) = dirs::home_dir() else {
            return;
        };
        let c = config_with_routes(vec![route("r", &["*.pdf"], "~/refs")]);
        assert_eq!(
            c.route_for("a.pdf").unwrap().destination(),
            home.join("refs")
        );
    }

    #[test]
    fn target_name_renders_stem_date_and_ext() {
        let mut r = route("r", &["*persona*"], "/dest");
        r.rename = Some("{stem}-{date}.{ext}".into());
        r.date_source = DateSource::Stem;
        let resolved = ResolvedRoute {
            name: r.name,
            patterns: r.patterns,
            destination: PathBuf::from("/dest"),
            rename: r.rename,
            date_source: r.date_source,
        };
        assert_eq!(
            resolved.target_name("report-20260330", "md", Path::new("/nope"), "fallback.md"),
            "report-20260330-2026-03-30.md"
        );
    }

    #[test]
    fn target_name_falls_back_when_no_template_or_no_date() {
        let no_template = ResolvedRoute {
            name: "r".into(),
            patterns: vec![],
            destination: PathBuf::from("/dest"),
            rename: None,
            date_source: DateSource::Mtime,
        };
        assert_eq!(
            no_template.target_name("x", "md", Path::new("/nope"), "keep.md"),
            "keep.md"
        );

        let mut needs_date = route("r", &["*"], "/dest");
        needs_date.rename = Some("{date}.md".into());
        needs_date.date_source = DateSource::Stem;
        let resolved = ResolvedRoute {
            name: needs_date.name,
            patterns: needs_date.patterns,
            destination: PathBuf::from("/dest"),
            rename: needs_date.rename,
            date_source: needs_date.date_source,
        };
        assert_eq!(
            resolved.target_name("nodate", "md", Path::new("/nope"), "keep.md"),
            "keep.md"
        );
    }
}
