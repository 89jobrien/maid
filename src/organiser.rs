//! Scans files, previews and applies organization rules, converts documents, and supports undo.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::error::MaidError;

pub struct FileEntry {
    pub path: PathBuf,
    pub folder: String,
}

#[derive(Serialize, Deserialize)]
pub struct LogEntry {
    pub from: String,
    pub to: String,
}

const LOG_FILE: &str = ".maid_log.json";
const NOTES_EXTENSIONS: &[&str] = &["md", "mdx"];
const SECS_PER_DAY: u64 = 86400;
const MAX_DISAMBIGUATION_ATTEMPTS: u32 = 1000;
const AUDIT_SUMMARY_PREFIX: &str = "Audit report:";
const AUDIT_TOTAL_MARKER: &str = "total match(es)";

/// Collects visible files in a directory and classifies them by extension.
pub fn scan(dir: &Path, config: &Config) -> Result<Vec<FileEntry>, MaidError> {
    if !dir.is_dir() {
        return Err(MaidError::InvalidDirectory(
            dir.to_string_lossy().to_string(),
        ));
    }

    let entries = fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_file())
        .filter(|e| {
            e.path()
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| !n.starts_with('.'))
                .unwrap_or(false)
        })
        .map(|e| {
            let path = e.path();
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            let folder = config.classify(ext).to_string();
            FileEntry { path, folder }
        })
        .collect();

    Ok(entries)
}

/// Prints the actions Maid would take without changing any files.
pub fn preview(entries: &[FileEntry], dir: &Path, config: &Config) {
    if entries.is_empty() {
        println!("Nothing to organise.");
        return;
    }

    println!("\nPreview - no files will be moved:\n");
    let mut noted = 0;

    for entry in entries {
        let filename = entry
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");

        let ext = entry
            .path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");

        let action = config.action_for(&entry.folder);

        if action == "note" {
            let age = file_age_days(&entry.path);
            let stale_threshold = config.stale_days(&entry.folder);
            let is_stale = stale_threshold.is_some_and(|t| age >= t);

            if is_stale {
                println!(" {} -> ARCHIVE (stale, {} days old)", filename, age);
            } else {
                println!(" {} [NOTED] ({} days old)", filename, age);
                noted += 1;
            }
            continue;
        }

        if let Some(tool) = config.converter_for(ext) {
            let md_name = swap_ext(filename, "md");
            println!(
                " {} -> CONVERT ({}) -> {} + archive original",
                filename, tool, md_name
            );
        } else if is_notes_ext(ext) && !obfsck_check(&entry.path) {
            println!(
                " {} -> QUARANTINE ({})",
                filename,
                config.quarantine_dir().display()
            );
        } else {
            let dest = config.destination(&entry.folder, dir);
            println!(" {} -> {}", filename, dest.display());
        }
    }

    let actionable = entries.len() - noted;
    println!(
        "\n{} file(s) would be processed, {} noted in place.",
        actionable, noted
    );
}

/// Applies configured move, note, conversion, quarantine, and archive actions.
pub fn organise(dir: &Path, entries: &[FileEntry], config: &Config) -> Result<(), MaidError> {
    let mut log: Vec<LogEntry> = Vec::new();
    let mut moved = 0;
    let mut converted = 0;
    let mut quarantined = 0;
    let mut noted = 0;
    let mut stale_archived = 0;

    for entry in entries {
        let filename_os = entry
            .path
            .file_name()
            .ok_or_else(|| MaidError::Io(std::io::Error::other("Invalid filename")))?;
        let filename = filename_os.to_string_lossy();

        let ext = entry
            .path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");

        let action = config.action_for(&entry.folder);

        // --- Note-only categories (e.g. diagnostics) ---
        if action == "note" {
            let age = file_age_days(&entry.path);
            let stale_threshold = config.stale_days(&entry.folder);
            let is_stale = stale_threshold.is_some_and(|t| age >= t);

            if is_stale {
                let archive_dir = config.archive_dir();
                if !archive_dir.exists() {
                    fs::create_dir_all(&archive_dir)?;
                }
                let now = Utc::now().format("%Y%m%d");
                let archived_name = format!("{}-{}", now, filename);
                let archive_dest = resolve_destination(archive_dir.join(&archived_name))?;

                log.push(LogEntry {
                    from: entry.path.to_string_lossy().to_string(),
                    to: archive_dest.to_string_lossy().to_string(),
                });

                fs::rename(&entry.path, &archive_dest)?;
                println!(" {} -> ARCHIVED (stale, {} days old)", filename, age);
                stale_archived += 1;
            } else {
                println!(" {} [NOTED] ({} days old)", filename, age);
                noted += 1;
            }
            continue;
        }

        // --- Conversion pipeline ---
        if let Some(tool) = config.converter_for(ext) {
            let md_name = swap_ext(&filename, "md");
            let md_path = entry.path.with_file_name(&md_name);

            let ok = convert_file(tool, &entry.path, &md_path);
            if !ok {
                eprintln!(" FAILED to convert: {}", filename);
                continue;
            }

            inject_frontmatter(&md_path, dir, Some(&entry.path))?;

            let is_clean = obfsck_check(&md_path);
            let md_dest_dir = if is_clean {
                config.destination("notes", dir)
            } else {
                quarantined += 1;
                config.quarantine_dir()
            };

            if !md_dest_dir.exists() {
                fs::create_dir_all(&md_dest_dir)?;
            }

            let md_destination = resolve_destination(md_dest_dir.join(&md_name))?;
            fs::rename(&md_path, &md_destination)?;

            let archive_dir = config.archive_dir();
            if !archive_dir.exists() {
                fs::create_dir_all(&archive_dir)?;
            }
            let now = Utc::now().format("%Y%m%d");
            let archived_name = format!("{}-{}", now, filename);
            let archive_dest = resolve_destination(archive_dir.join(&archived_name))?;
            fs::rename(&entry.path, &archive_dest)?;

            log.push(LogEntry {
                from: entry.path.to_string_lossy().to_string(),
                to: archive_dest.to_string_lossy().to_string(),
            });
            log.push(LogEntry {
                from: "(converted)".to_string(),
                to: md_destination.to_string_lossy().to_string(),
            });

            if is_clean {
                println!(
                    " {} -> {} (converted) + archived",
                    filename,
                    md_dest_dir.display()
                );
            } else {
                eprintln!(" {} -> QUARANTINED (converted, secrets detected)", filename);
            }

            converted += 1;
            continue;
        }

        // --- Notes: obfsck gate ---
        let is_quarantined = is_notes_ext(ext) && !obfsck_check(&entry.path);

        let dest_dir = if is_quarantined {
            config.quarantine_dir()
        } else {
            config.destination(&entry.folder, dir)
        };

        if !dest_dir.exists() {
            fs::create_dir_all(&dest_dir)?;
        }

        if is_notes_ext(ext) && !is_quarantined {
            inject_frontmatter(&entry.path, dir, None)?;
        }

        let destination = resolve_destination(dest_dir.join(filename_os))?;

        log.push(LogEntry {
            from: entry.path.to_string_lossy().to_string(),
            to: destination.to_string_lossy().to_string(),
        });

        fs::rename(&entry.path, &destination)?;

        if is_quarantined {
            eprintln!(" {} -> QUARANTINED ({})", filename, dest_dir.display());
            quarantined += 1;
        } else {
            println!(" {} -> {}", filename, dest_dir.display());
            moved += 1;
        }
    }

    // Write undo log
    let log_path = dir.join(LOG_FILE);
    let log_contents =
        serde_json::to_string_pretty(&log).map_err(|e| MaidError::UndoFailed(e.to_string()))?;
    fs::write(log_path, log_contents)?;

    println!(
        "\n{} moved, {} converted, {} quarantined, {} noted, {} stale archived.",
        moved, converted, quarantined, noted, stale_archived
    );

    Ok(())
}

/// Reverses the moves recorded by the directory's most recent Maid run.
pub fn undo(dir: &Path) -> Result<(), MaidError> {
    let log_path = dir.join(LOG_FILE);

    if !log_path.exists() {
        return Err(MaidError::UndoFailed(
            "No undo log found. Has maid been run here?".to_string(),
        ));
    }

    let contents = fs::read_to_string(&log_path)?;
    let log: Vec<LogEntry> =
        serde_json::from_str(&contents).map_err(|e| MaidError::UndoFailed(e.to_string()))?;

    for entry in &log {
        if entry.from == "(converted)" {
            let _ = fs::remove_file(&entry.to);
            println!(" Removed converted: {}", entry.to);
        } else {
            fs::rename(&entry.to, &entry.from)?;
            println!(" Restored: {}", entry.from);
        }
    }

    let category_dirs = log
        .iter()
        .filter_map(|e| Path::new(&e.to).parent().map(|p| p.to_path_buf()))
        .collect::<HashSet<_>>();

    for folder in category_dirs {
        if folder != dir {
            let _ = fs::remove_dir(&folder);
        }
    }

    fs::remove_file(&log_path)?;
    println!("\n{} action(s) undone.", log.len());

    Ok(())
}

fn is_notes_ext(ext: &str) -> bool {
    NOTES_EXTENSIONS.contains(&ext.to_lowercase().as_str())
}

fn file_age_days(path: &Path) -> u64 {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|mtime| SystemTime::now().duration_since(mtime).ok())
        .map(|d| d.as_secs() / SECS_PER_DAY)
        .unwrap_or(0)
}

/// Extracts the total match count from an `obfsck redact --audit` report.
///
/// obfsck writes the summary to stderr as:
/// `Audit report: <n> pattern type(s), <m> total match(es)`
///
/// Returns `None` when the summary is absent or malformed, which callers treat
/// as "no verdict available" rather than as clean or dirty.
fn audit_match_count(report: &str) -> Option<usize> {
    let summary = report
        .lines()
        .find(|line| line.trim_start().starts_with(AUDIT_SUMMARY_PREFIX))?;
    let (counts, _) = summary.split_once(AUDIT_TOTAL_MARKER)?;
    let digits: String = counts
        .trim_end()
        .chars()
        .rev()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.chars().rev().collect::<String>().parse().ok()
}

/// Secret-scans a file, returning true when it is safe to keep.
///
/// Detection reads the match count from `obfsck redact --audit`. obfsck's
/// `redact` exit code is always 0 whether or not it finds anything, so it
/// carries no verdict, and comparing redacted output against the original is
/// unreliable because obfsck normalises trailing newlines and CRLF endings.
///
/// Every failure to obtain a verdict — obfsck missing, non-zero exit, or an
/// unreadable report — is reported as clean with a warning. Treating a tool
/// error as "dirty" quarantines every note, which is the failure this
/// integration previously had.
fn obfsck_check(path: &Path) -> bool {
    let Ok(output) = Command::new("obfsck")
        .arg("redact")
        .arg("--audit")
        .arg(path)
        .output()
    else {
        eprintln!(" WARNING: obfsck not found, skipping secret check");
        return true;
    };

    if !output.status.success() {
        eprintln!(
            " WARNING: obfsck exited {} on {}, skipping secret check",
            output.status,
            path.display()
        );
        return true;
    }

    let report = String::from_utf8_lossy(&output.stderr);
    match audit_match_count(&report) {
        Some(0) => true,
        Some(total) => {
            eprintln!(" {} secret match(es) in {}", total, path.display());
            false
        }
        None => {
            eprintln!(
                " WARNING: no obfsck audit report for {}, skipping secret check",
                path.display()
            );
            true
        }
    }
}

fn inject_frontmatter(
    path: &Path,
    source_dir: &Path,
    converted_from: Option<&Path>,
) -> Result<(), MaidError> {
    let content = fs::read_to_string(path)?;

    if content.starts_with("---\n") {
        return Ok(());
    }

    let source_dir_name = source_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");

    let now = Utc::now().format("%Y-%m-%dT%H:%M:%SZ");

    let converted_line = match converted_from {
        Some(orig) => format!("converted_from: {}\n", orig.display()),
        None => String::new(),
    };

    let frontmatter = format!(
        "---\nsource: {}\nmoved_by: maid\nmoved_at: {}\noriginal_dir: {}\n{}---\n\n",
        path.display(),
        now,
        source_dir_name,
        converted_line,
    );

    fs::write(path, format!("{}{}", frontmatter, content))?;

    Ok(())
}

fn convert_file(tool: &str, input: &Path, output: &Path) -> bool {
    match tool {
        "marker" => convert_with_marker(input, output),
        "mutool" => convert_with_mutool(input, output),
        _ => convert_with_pandoc(input, output),
    }
}

fn convert_with_pandoc(input: &Path, output: &Path) -> bool {
    match Command::new("pandoc")
        .arg(input)
        .arg("-t")
        .arg("markdown")
        .arg("-o")
        .arg(output)
        .output()
    {
        Ok(out) => {
            if !out.status.success() {
                eprintln!(" pandoc error: {}", String::from_utf8_lossy(&out.stderr));
            }
            out.status.success()
        }
        Err(e) => {
            eprintln!(" Failed to run pandoc: {}", e);
            false
        }
    }
}

/// marker_single writes output to <input_dir>/<stem>/<stem>.md
/// We run it, then move the .md to the expected output path and clean up.
fn convert_with_marker(input: &Path, output: &Path) -> bool {
    let result = Command::new("marker_single")
        .arg(input)
        .arg("--disable_image_extraction")
        .output();

    match result {
        Ok(out) => {
            if !out.status.success() {
                eprintln!(
                    " marker_single error: {}",
                    String::from_utf8_lossy(&out.stderr)
                );
                return false;
            }

            // marker_single creates <parent>/<stem>/<stem>.md
            let stem = input.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            let parent = input.parent().unwrap_or(Path::new("."));
            let marker_dir = parent.join(stem);
            let marker_md = marker_dir.join(format!("{}.md", stem));

            if marker_md.exists() {
                if let Err(e) = fs::rename(&marker_md, output) {
                    eprintln!(" Failed to move marker output: {}", e);
                    return false;
                }
                // Clean up the marker output directory
                let _ = fs::remove_dir_all(&marker_dir);
                true
            } else {
                eprintln!(" marker_single produced no output for {}", stem);
                // Clean up if directory was created
                let _ = fs::remove_dir_all(&marker_dir);
                false
            }
        }
        Err(e) => {
            eprintln!(" Failed to run marker_single: {}", e);
            false
        }
    }
}

/// Use mutool to extract text from PDF and wrap as markdown.
fn convert_with_mutool(input: &Path, output: &Path) -> bool {
    let result = Command::new("mutool")
        .args(["convert", "-F", "text", "-o", "-"])
        .arg(input)
        .output();

    match result {
        Ok(out) => {
            if !out.status.success() {
                eprintln!(" mutool error: {}", String::from_utf8_lossy(&out.stderr));
                return false;
            }
            let text = String::from_utf8_lossy(&out.stdout);
            match fs::write(output, text.as_ref()) {
                Ok(()) => true,
                Err(e) => {
                    eprintln!(" Failed to write mutool output: {}", e);
                    false
                }
            }
        }
        Err(e) => {
            eprintln!(" Failed to run mutool: {}", e);
            false
        }
    }
}

fn swap_ext(filename: &str, new_ext: &str) -> String {
    match filename.rsplit_once('.') {
        Some((stem, _)) => format!("{}.{}", stem, new_ext),
        None => format!("{}.{}", filename, new_ext),
    }
}

/// Splits `name` into (stem, extension) for suffix insertion.
///
/// A name with no dot yields `(name, "")`. A pure dotfile such as
/// `.gitignore` has no stem, so it is returned whole as the stem — this
/// keeps `.gitignore` from becoming `-1.gitignore`.
fn split_name(name: &str) -> (&str, &str) {
    match name.rsplit_once('.') {
        Some(("", _)) => (name, ""),
        Some((stem, ext)) => (stem, ext),
        None => (name, ""),
    }
}

/// Builds the `n`-th disambiguated candidate for `stem`/`ext`.
fn disambiguated(stem: &str, ext: &str, n: u32) -> String {
    if ext.is_empty() {
        format!("{}-{}", stem, n)
    } else {
        format!("{}-{}.{}", stem, n, ext)
    }
}

/// Resolves `desired` to a path that does not exist by inserting a `-N`
/// suffix before the extension, incrementing from 1.
///
/// Fast path: returns `desired` unchanged when that path is already free,
/// so non-colliding files keep byte-identical destinations.
///
/// Returns `MaidError::DestinationExhausted` when
/// `MAX_DISAMBIGUATION_ATTEMPTS` candidates are exhausted. Never falls back
/// to overwriting.
fn resolve_destination(desired: PathBuf) -> Result<PathBuf, MaidError> {
    if !desired.exists() {
        return Ok(desired);
    }

    let parent = desired
        .parent()
        .map_or_else(PathBuf::new, Path::to_path_buf);
    let name = desired
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (stem, ext) = split_name(&name);

    for n in 1..=MAX_DISAMBIGUATION_ATTEMPTS {
        let candidate = parent.join(disambiguated(stem, ext, n));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }

    Err(MaidError::DestinationExhausted(format!(
        "no free name for '{}' in '{}' after {} attempts",
        name,
        parent.display(),
        MAX_DISAMBIGUATION_ATTEMPTS
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn swap_ext_replaces_final_extension() {
        assert_eq!(swap_ext("invoice.pdf", "md"), "invoice.md");
        assert_eq!(swap_ext("a.b.pdf", "md"), "a.b.md");
    }

    #[test]
    fn swap_ext_appends_when_no_extension() {
        assert_eq!(swap_ext("README", "md"), "README.md");
    }

    #[test]
    fn notes_extensions_are_recognised() {
        assert!(is_notes_ext("md"));
        assert!(is_notes_ext("mdx"));
        assert!(is_notes_ext("MD"));
        assert!(!is_notes_ext("txt"));
        assert!(!is_notes_ext(""));
    }

    #[test]
    fn log_entry_round_trips_through_json() {
        let entries = vec![
            LogEntry {
                from: "/dl/invoice.pdf".into(),
                to: "/archive/20260927-invoice.pdf".into(),
            },
            LogEntry {
                from: "(converted)".into(),
                to: "/notes/invoice.md".into(),
            },
        ];
        let json = serde_json::to_string(&entries).expect("serialise log");
        let back: Vec<LogEntry> = serde_json::from_str(&json).expect("deserialise log");

        assert_eq!(back.len(), 2);
        assert_eq!(back[0].from, "/dl/invoice.pdf");
        assert_eq!(back[0].to, "/archive/20260927-invoice.pdf");
        assert_eq!(back[1].from, "(converted)");
        assert_eq!(back[1].to, "/notes/invoice.md");
    }

    #[test]
    fn scan_rejects_missing_directory() {
        let config = crate::config::Config::defaults();
        let result = scan(Path::new("/definitely/not/here/maid-test"), &config);
        assert!(matches!(result, Err(MaidError::InvalidDirectory(_))));
    }

    #[test]
    fn scan_skips_hidden_files_and_subdirectories() {
        let dir = std::env::temp_dir().join("maid-scan-hidden-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("subdir")).expect("create temp dir");
        fs::write(dir.join("visible.jpg"), b"x").expect("write visible file");
        fs::write(dir.join(".hidden.jpg"), b"x").expect("write hidden file");
        fs::write(dir.join("subdir/nested.jpg"), b"x").expect("write nested file");

        let config = crate::config::Config::defaults();
        let entries = scan(&dir, &config).expect("scan succeeds");

        let names: Vec<String> = entries
            .iter()
            .map(|e| {
                e.path
                    .file_name()
                    .expect("name")
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();

        assert_eq!(names, vec!["visible.jpg".to_string()]);
        assert_eq!(entries[0].folder, "images");

        fs::remove_dir_all(&dir).expect("clean up temp dir");
    }

    #[test]
    fn file_age_days_is_zero_for_missing_file() {
        assert_eq!(
            file_age_days(Path::new("/definitely/not/here/maid-test")),
            0
        );
    }

    #[test]
    fn audit_match_count_reads_clean_report() {
        let report = "Audit report: 0 pattern type(s), 0 total match(es)\n";
        assert_eq!(audit_match_count(report), Some(0));
    }

    #[test]
    fn audit_match_count_reads_dirty_report() {
        let report = "Audit report: 1 pattern type(s), 1 total match(es)\n  \
                      [REDACTED-AWS-KEY]                  1\n";
        assert_eq!(audit_match_count(report), Some(1));
    }

    #[test]
    fn audit_match_count_reads_multi_digit_total() {
        let report = "Audit report: 3 pattern type(s), 42 total match(es)\n";
        assert_eq!(audit_match_count(report), Some(42));
    }

    #[test]
    fn audit_match_count_ignores_absence_of_a_report() {
        assert_eq!(audit_match_count(""), None);
        assert_eq!(audit_match_count("some unrelated stderr\n"), None);
    }

    #[test]
    fn audit_match_count_rejects_malformed_total() {
        // Present but unparseable: must be None, never a fabricated zero.
        assert_eq!(
            audit_match_count("Audit report: many total match(es)\n"),
            None
        );
    }

    #[test]
    fn audit_match_count_tolerates_a_summary_without_the_total_marker() {
        assert_eq!(audit_match_count("Audit report: 0 pattern type(s)\n"), None);
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("maid-test-{}-{}", tag, std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn cleanup(dir: &Path) {
        fs::remove_dir_all(dir).expect("clean up temp dir");
    }

    fn notes_config(dest: &Path) -> crate::config::Config {
        let categories = HashMap::from([("notes".to_string(), vec!["md".to_string()])]);
        let destinations = HashMap::from([("notes".to_string(), dest.to_path_buf())]);
        crate::config::test_config_with(categories, destinations)
    }

    #[test]
    fn colliding_names_across_source_directories_both_survive() {
        let root = temp_dir("collision");
        let a = root.join("a");
        let b = root.join("b");
        let inbox = root.join("inbox");
        fs::create_dir_all(&a).expect("a");
        fs::create_dir_all(&b).expect("b");
        fs::write(a.join("collision.md"), b"# From A\n").expect("a file");
        fs::write(b.join("collision.md"), b"# From B\n").expect("b file");

        let config = notes_config(&inbox);

        let entries_a = scan(&a, &config).expect("scan a");
        organise(&a, &entries_a, &config).expect("organise a");
        let entries_b = scan(&b, &config).expect("scan b");
        organise(&b, &entries_b, &config).expect("organise b");

        let names: Vec<String> = fs::read_dir(&inbox)
            .expect("read inbox")
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names.len(), 2, "both files must survive: {:?}", names);

        undo(&a).expect("undo a");
        undo(&b).expect("undo b");

        assert!(a.join("collision.md").exists(), "A must be restored");
        assert!(b.join("collision.md").exists(), "B must be restored");

        // Restored content carries maid's provenance frontmatter, so assert
        // identity rather than byte equality: each file must come back to its
        // own origin directory with its own body. Before the collision fix
        // this returned B's body under A's path.
        let a_text = fs::read_to_string(a.join("collision.md")).expect("read a");
        let b_text = fs::read_to_string(b.join("collision.md")).expect("read b");

        assert!(a_text.contains("# From A"), "A must hold A's body");
        assert!(!a_text.contains("# From B"), "A must not hold B's body");
        assert!(a_text.contains("original_dir: a"), "A keeps its own origin");

        assert!(b_text.contains("# From B"), "B must hold B's body");
        assert!(!b_text.contains("# From A"), "B must not hold A's body");
        assert!(b_text.contains("original_dir: b"), "B keeps its own origin");

        cleanup(&root);
    }

    #[test]
    fn resolve_destination_returns_free_path_unchanged() {
        let dir = temp_dir("resolve-fast");
        let desired = dir.join("invoice.md");
        let resolved = resolve_destination(desired.clone()).expect("resolves");
        assert_eq!(resolved, desired);
        cleanup(&dir);
    }

    #[test]
    fn resolve_destination_suffixes_before_extension() {
        let dir = temp_dir("resolve-suffix");
        fs::write(dir.join("invoice.md"), b"x").expect("seed");
        let resolved = resolve_destination(dir.join("invoice.md")).expect("resolves");
        assert_eq!(resolved.file_name().expect("name"), "invoice-1.md");
        cleanup(&dir);
    }

    #[test]
    fn resolve_destination_increments_past_existing_candidates() {
        let dir = temp_dir("resolve-increment");
        fs::write(dir.join("invoice.md"), b"x").expect("seed");
        fs::write(dir.join("invoice-1.md"), b"x").expect("seed");
        let resolved = resolve_destination(dir.join("invoice.md")).expect("resolves");
        assert_eq!(resolved.file_name().expect("name"), "invoice-2.md");
        cleanup(&dir);
    }

    #[test]
    fn resolve_destination_handles_names_without_extension() {
        let dir = temp_dir("resolve-noext");
        fs::write(dir.join("README"), b"x").expect("seed");
        let resolved = resolve_destination(dir.join("README")).expect("resolves");
        assert_eq!(resolved.file_name().expect("name"), "README-1");
        cleanup(&dir);
    }

    #[test]
    fn resolve_destination_treats_dotfile_as_stem() {
        let dir = temp_dir("resolve-dotfile");
        fs::write(dir.join(".gitignore"), b"x").expect("seed");
        let resolved = resolve_destination(dir.join(".gitignore")).expect("resolves");
        assert_eq!(resolved.file_name().expect("name"), ".gitignore-1");
        cleanup(&dir);
    }

    #[test]
    fn resolve_destination_errors_when_candidates_are_exhausted() {
        let dir = temp_dir("resolve-exhaust");
        fs::write(dir.join("invoice.md"), b"x").expect("seed");
        for n in 1..=MAX_DISAMBIGUATION_ATTEMPTS {
            fs::write(dir.join(format!("invoice-{}.md", n)), b"x").expect("seed");
        }
        let result = resolve_destination(dir.join("invoice.md"));
        assert!(matches!(result, Err(MaidError::DestinationExhausted(_))));
        cleanup(&dir);
    }
}
