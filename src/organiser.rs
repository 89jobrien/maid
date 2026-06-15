use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

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

pub fn preview(entries: &[FileEntry], dir: &Path, config: &Config) {
    if entries.is_empty() {
        println!("Nothing to organise.");
        return;
    }

    println!("\nPreview - no files will be moved:\n");
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

    println!("\n{} file(s) would be processed.", entries.len());
}

pub fn organise(dir: &Path, entries: &[FileEntry], config: &Config) -> Result<(), MaidError> {
    let mut log: Vec<LogEntry> = Vec::new();
    let mut moved = 0;
    let mut converted = 0;
    let mut quarantined = 0;

    for entry in entries {
        let filename_os = entry.path.file_name().ok_or_else(|| {
            MaidError::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                "Invalid filename",
            ))
        })?;
        let filename = filename_os.to_string_lossy();

        let ext = entry
            .path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");

        // --- Conversion pipeline ---
        if let Some(tool) = config.converter_for(ext) {
            let md_name = swap_ext(&filename, "md");
            let md_path = entry.path.with_file_name(&md_name);

            // Convert
            let ok = convert_file(tool, &entry.path, &md_path);
            if !ok {
                eprintln!(" FAILED to convert: {}", filename);
                continue;
            }

            // Inject frontmatter on the new .md
            inject_frontmatter(&md_path, dir, Some(&entry.path))?;

            // obfsck check on the converted markdown
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

            let md_destination = md_dest_dir.join(&md_name);
            fs::rename(&md_path, &md_destination)?;

            // Archive the original
            let archive_dir = config.archive_dir();
            if !archive_dir.exists() {
                fs::create_dir_all(&archive_dir)?;
            }
            let now = Utc::now().format("%Y%m%d");
            let archived_name = format!("{}-{}", now, filename);
            let archive_dest = archive_dir.join(&archived_name);
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

        // Inject frontmatter for clean markdown files
        if is_notes_ext(ext) && !is_quarantined {
            inject_frontmatter(&entry.path, dir, None)?;
        }

        let destination = dest_dir.join(filename_os);

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
        "\n{} moved, {} converted, {} quarantined.",
        moved, converted, quarantined
    );

    Ok(())
}

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
            // Remove converted file, original was archived
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

fn obfsck_check(path: &Path) -> bool {
    let result = Command::new("obfsck").arg("check").arg(path).output();

    match result {
        Ok(output) => output.status.success(),
        Err(_) => {
            eprintln!(" WARNING: obfsck not found, skipping secret check");
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
    let result = match tool {
        "marker" => Command::new("marker")
            .arg(input)
            .arg("-o")
            .arg(output)
            .output(),
        "pandoc" | _ => Command::new("pandoc")
            .arg(input)
            .arg("-t")
            .arg("markdown")
            .arg("-o")
            .arg(output)
            .output(),
    };

    match result {
        Ok(out) => {
            if !out.status.success() {
                eprintln!(" {} error: {}", tool, String::from_utf8_lossy(&out.stderr));
            }
            out.status.success()
        }
        Err(e) => {
            eprintln!(" Failed to run {}: {}", tool, e);
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
