# Plan: Collision-Safe Destination Resolution and Durable Undo Logging

Design: `docs/designs/2026-09-28-collision-safe-destination-resolution-design.md`

## Goal

Maid must never silently destroy a file, and every move it performs must remain reversible even when a run fails partway through.

## Architecture

- **Crates affected**: `maid` only. Binary-only crate, no `lib.rs`, no public API surface.
- **New types**: none. `LogEntry` (`src/organiser.rs:21`) is unchanged — JSONL is a framing change, not a schema change.
- **New error variant**: `MaidError::DestinationExhausted(String)` in `src/error.rs`.
- **New constants** in `src/organiser.rs`: `JOURNAL_FILE`, `LEGACY_LOG_FILE`, `MAX_DISAMBIGUATION_ATTEMPTS`. `LOG_FILE` is removed.
- **New functions** in `src/organiser.rs`: `resolve_destination`, `append_log_entry`, `read_log`, `restore_entry`, plus private helpers `split_name`, `disambiguated`.
- **Changed signature**: `organiser::preview` gains `-> Result<(), MaidError>`; its single caller at `src/main.rs:73` gains `?`.
- **Data flow**: `scan` → `FileEntry` → classify → action → build desired path → `resolve_destination` → `fs::rename` → `append_log_entry`.
- **No new dependencies. No config schema change. No hexagonal ports** — deliberately; rationale in the design doc.

## Tech Stack

- Rust edition 2024, MSRV 1.85 (currently unpinned in `Cargo.toml`)
- Existing deps only: `clap`, `serde`, `serde_json`, `toml`, `dirs`, `chrono`
- Tests: in-source `#[cfg(test)] mod tests`, driven by `cargo nextest run --workspace`

---

## Task 1: Add `DestinationExhausted` error variant

**Crate**: `maid`
**File(s)**: `src/error.rs`
**Run**: `cargo nextest run -p maid -- destination_exhausted`

Create branch first — this work is unrelated to the obfsck fix already on `fix/obfsck-secret-detection`:

```bash
git switch -c fix/collision-safe-destinations
git branch --show-current   # must print fix/collision-safe-destinations
```

1. Write the failing test. Append to `src/error.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn destination_exhausted_renders_reason() {
        let err = MaidError::DestinationExhausted("no free name for 'invoice.md'".into());
        assert_eq!(
            err.to_string(),
            "Destination exhausted: no free name for 'invoice.md'"
        );
    }

    #[test]
    fn invalid_directory_renders_path() {
        let err = MaidError::InvalidDirectory("/nope".into());
        assert_eq!(err.to_string(), "Invalid directory: /nope");
    }
}
```

2. Run `cargo nextest run -p maid -- destination_exhausted` → expected **FAIL** (cannot find `MaidError::DestinationExhausted`)

3. Implement. Add the variant to the enum and the arm to `Display`:

```rust
#[derive(Debug)]
pub enum MaidError {
    Io(std::io::Error),
    InvalidDirectory(String),
    UndoFailed(String),
    ConfigError(String),
    DestinationExhausted(String),
}
```

```rust
            MaidError::DestinationExhausted(reason) => {
                write!(f, "Destination exhausted: {}", reason)
            }
```

4. Verify:

```
cargo nextest run -p maid    → all green
cargo clippy -p maid -- -D warnings  → zero warnings
```

5. `git branch --show-current` → `fix/collision-safe-destinations`. Stop if not.
   Commit: `git commit -m "feat(maid): add DestinationExhausted error variant"`

---

## Task 2: Add test-only config builder

**Crate**: `maid`
**File(s)**: `src/config.rs`
**Run**: `cargo nextest run -p maid -- test_config_with`

`Config`'s fields are private to `config.rs`, so `organiser.rs` tests cannot construct a config with custom destinations. This adds a `#[cfg(test)]`-only constructor inside `config.rs`, where `Config::build` is reachable.

This is the one item in this plan that is **not** named in the design doc's API list. It is `#[cfg(test)]`-gated test scaffolding, not shipped API. Flag it at review.

1. Append inside `src/config.rs`, above the `mod tests`:

```rust
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
```

2. Verify `cargo nextest run -p maid` → still green, and `cargo clippy -p maid -- -D warnings` → zero warnings. (`HashMap` and `PathBuf` are already imported at `src/config.rs:3` and `:4`.)

3. `git branch --show-current` → `fix/collision-safe-destinations`.
   Commit: `git commit -m "test(maid): add cfg(test) config builder for organiser tests"`

---

## Task 3: Implement `resolve_destination`

**Crate**: `maid`
**File(s)**: `src/organiser.rs`
**Run**: `cargo nextest run -p maid -- resolve_destination`

1. Write the failing tests. Inside the existing `mod tests` in `src/organiser.rs`, add helpers then tests:

```rust
    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("maid-test-{}-{}", tag, std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn cleanup(dir: &Path) {
        fs::remove_dir_all(dir).expect("clean up temp dir");
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
```

2. Run `cargo nextest run -p maid -- resolve_destination` → expected **FAIL** (`cannot find function resolve_destination`)

3. Implement. Add the constant next to the existing ones at `src/organiser.rs:26-28`:

```rust
const MAX_DISAMBIGUATION_ATTEMPTS: u32 = 1000;
```

Add the functions after `swap_ext`:

```rust
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
```

4. Verify:

```
cargo nextest run -p maid -- resolve_destination    → 6 passed
cargo nextest run -p maid                          → all green
cargo clippy -p maid -- -D warnings               → zero warnings
```

5. `git branch --show-current` → `fix/collision-safe-destinations`.
   Commit: `git commit -m "feat(organiser): add collision-safe destination resolver"`

---

## Task 4: Wire the resolver into all four write sites

**Crate**: `maid`
**File(s)**: `src/organiser.rs`
**Run**: `cargo nextest run -p maid -- colliding_names`

1. Write the failing regression test. This is the scenario that destroyed data. Add inside `mod tests`:

```rust
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
        assert_eq!(
            fs::read_to_string(a.join("collision.md")).expect("read a"),
            "# From A\n"
        );
        assert_eq!(
            fs::read_to_string(b.join("collision.md")).expect("read b"),
            "# From B\n"
        );

        cleanup(&root);
    }
```

Add `use std::collections::HashMap;` to the test module.

2. Run `cargo nextest run -p maid -- colliding_names` → expected **FAIL** (`only 1 file survives: ["collision.md"]`)

3. Implement. Four edits in `organise`:

Site 1 — stale-note archive, `src/organiser.rs:161-162`:

```rust
                let archive_dest = resolve_destination(archive_dir.join(&archived_name))?;
```

Site 2 — converted-output move, `src/organiser.rs:204`:

```rust
            let md_destination = resolve_destination(md_dest_dir.join(&md_name))?;
```

Site 3 — converted-original archive, `src/organiser.rs:213`:

```rust
            let archive_dest = resolve_destination(archive_dir.join(&archived_name))?;
```

Site 4 — primary move and quarantine, `src/organiser.rs:256`:

```rust
        let destination = resolve_destination(dest_dir.join(filename_os))?;
```

**Do not touch `src/organiser.rs:502`** — that is inside `convert_with_marker`, relocating `marker_single`'s own output. It is not a maid destination.

4. Verify:

```
cargo nextest run -p maid -- colliding_names    → 1 passed
cargo nextest run -p maid                      → all green
cargo clippy -p maid -- -D warnings           → zero warnings
```

5. `git branch --show-current` → `fix/collision-safe-destinations`.
   Commit: `git commit -m "fix(organiser): resolve collisions at every destination write site"`

---

## Task 5: Append-only JSONL journal

**Crate**: `maid`
**File(s)**: `src/organiser.rs`
**Run**: `cargo nextest run -p maid -- journal`

1. Write the failing tests:

```rust
    #[cfg(unix)]
    #[test]
    fn journal_records_completed_moves_when_a_later_move_fails() {
        use std::os::unix::fs::PermissionsExt;

        let root = temp_dir("durable");
        let src = root.join("src");
        let good = root.join("good");
        let locked = root.join("locked");
        fs::create_dir_all(&src).expect("src");
        fs::create_dir_all(&good).expect("good");
        fs::create_dir_all(&locked).expect("locked");
        fs::write(src.join("ok.md"), b"# Ok\n").expect("ok file");
        fs::write(src.join("blocked.md"), b"# Blocked\n").expect("blocked file");

        // r-x: readable and traversable, not writable. The move into it
        // fails with EACCES after the first file has already succeeded.
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o500)).expect("lock dir");

        let categories = HashMap::from([
            ("ok".to_string(), vec!["ok.md".to_string()]),
            ("blocked".to_string(), vec!["blocked.md".to_string()]),
        ]);
        let destinations = HashMap::from([
            ("ok".to_string(), good.clone()),
            ("blocked".to_string(), locked.clone()),
        ]);
        let config = crate::config::test_config_with(categories, destinations);

        let entries = scan(&src, &config).expect("scan");
        let result = organise(&src, &entries, &config);
        assert!(result.is_err(), "the locked move must fail");

        let (log_path, log) = read_log(&src).expect("journal readable");
        assert_eq!(log.len(), 1, "the completed move must be recorded");
        assert_eq!(log[0].from, src.join("ok.md").to_string_lossy());

        fs::set_permissions(&locked, fs::Permissions::from_mode(0o700)).expect("unlock");
        fs::remove_file(&log_path).expect("drop journal");
        cleanup(&root);
    }

    #[test]
    fn journal_round_trips_one_entry_per_line() {
        let dir = temp_dir("journal-roundtrip");
        let entries = vec![
            LogEntry {
                from: "/dl/a.md".into(),
                to: "/inbox/a.md".into(),
            },
            LogEntry {
                from: "(converted)".into(),
                to: "/inbox/b.md".into(),
            },
        ];
        for entry in &entries {
            append_log_entry(&dir, entry).expect("append");
        }
        let (path, back) = read_log(&dir).expect("read");
        assert_eq!(path, dir.join(JOURNAL_FILE));
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].from, "/dl/a.md");
        assert_eq!(back[1].from, "(converted)");
        cleanup(&dir);
    }

    #[test]
    fn read_log_skips_a_torn_trailing_line() {
        let dir = temp_dir("journal-torn");
        let good = LogEntry {
            from: "/dl/a.md".into(),
            to: "/inbox/a.md".into(),
        };
        append_log_entry(&dir, &good).expect("append good");
        let path = dir.join(JOURNAL_FILE);
        let mut contents = fs::read_to_string(&path).expect("read");
        contents.push_str("{\"from\":\"/dl/b.md\",\"to\":\"/inbo");
        fs::write(&path, contents).expect("append torn");

        let (_, back) = read_log(&dir).expect("read");
        assert_eq!(back.len(), 1, "earlier entries must survive a torn tail");
        assert_eq!(back[0].from, "/dl/a.md");
        cleanup(&dir);
    }

    #[test]
    fn read_log_falls_back_to_the_legacy_array() {
        let dir = temp_dir("journal-legacy");
        let legacy = vec![LogEntry {
            from: "/dl/old.md".into(),
            to: "/inbox/old.md".into(),
        }];
        fs::write(
            dir.join(LEGACY_LOG_FILE),
            serde_json::to_string_pretty(&legacy).expect("serialise"),
        )
        .expect("write legacy");

        let (path, back) = read_log(&dir).expect("read");
        assert_eq!(path, dir.join(LEGACY_LOG_FILE));
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].from, "/dl/old.md");
        cleanup(&dir);
    }

    #[test]
    fn read_log_prefers_the_journal_over_the_legacy_file() {
        let dir = temp_dir("journal-both");
        append_log_entry(
            &dir,
            &LogEntry {
                from: "/dl/new.md".into(),
                to: "/inbox/new.md".into(),
            },
        )
        .expect("append");
        fs::write(
            dir.join(LEGACY_LOG_FILE),
            r#"[{"from":"/dl/old.md","to":"/inbox/old.md"}]"#,
        )
        .expect("write legacy");

        let (path, back) = read_log(&dir).expect("read");
        assert_eq!(path, dir.join(JOURNAL_FILE));
        assert_eq!(back[0].from, "/dl/new.md");
        cleanup(&dir);
    }
```

2. Run `cargo nextest run -p maid -- journal` → expected **FAIL** (`cannot find function append_log_entry`)

3. Implement.

Replace the constant at `src/organiser.rs:26`:

```rust
const JOURNAL_FILE: &str = ".maid_log.jsonl";
const LEGACY_LOG_FILE: &str = ".maid_log.json";
```

Add `use std::io::Write;` to the imports at `src/organiser.rs:3-9`.

Add after `resolve_destination`:

```rust
/// Appends one `LogEntry` as a single JSON line, creating the journal if
/// absent. Called immediately after each successful move.
fn append_log_entry(dir: &Path, entry: &LogEntry) -> Result<(), MaidError> {
    let line = serde_json::to_string(entry)?;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(JOURNAL_FILE))?;
    writeln!(file, "{}", line)?;
    Ok(())
}

/// Reads the undo journal for `dir`, preferring `JOURNAL_FILE` and falling
/// back to the legacy `LEGACY_LOG_FILE` array. Unparseable trailing lines are
/// skipped so a torn final append does not lose earlier entries.
///
/// Returns the file it actually read so the caller removes the right one.
fn read_log(dir: &Path) -> Result<(PathBuf, Vec<LogEntry>), MaidError> {
    let journal = dir.join(JOURNAL_FILE);
    if journal.exists() {
        let contents = fs::read_to_string(&journal)?;
        let mut entries = Vec::new();
        for line in contents.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Ok(entry) = serde_json::from_str::<LogEntry>(trimmed) {
                entries.push(entry);
            }
        }
        return Ok((journal, entries));
    }

    let legacy = dir.join(LEGACY_LOG_FILE);
    if legacy.exists() {
        let contents = fs::read_to_string(&legacy)?;
        let entries: Vec<LogEntry> = serde_json::from_str(&contents)
            .map_err(|e| MaidError::UndoFailed(e.to_string()))?;
        return Ok((legacy, entries));
    }

    Err(MaidError::UndoFailed(
        "No undo log found. Has maid been run here?".to_string(),
    ))
}
```

Then convert `organise` from in-memory accumulation to per-entry appends:

- Delete `let mut log: Vec<LogEntry> = Vec::new();` at `src/organiser.rs:127`
- Add a fresh-journal truncate immediately before the loop:

```rust
    // Start a fresh journal for this run. Required: append_log_entry opens
    // in append mode, so without this a run would inherit the previous run's
    // entries and undo would replay stale moves.
    fs::write(dir.join(JOURNAL_FILE), "")?;
```

- Replace each of the four `log.push(LogEntry { ... });` calls (`src/organiser.rs:164`, `:216`, `:220`, `:258`) with:

```rust
        append_log_entry(
            dir,
            &LogEntry {
                from: entry.path.to_string_lossy().to_string(),
                to: archive_dest.to_string_lossy().to_string(),
            },
        )?;
```

using the `to` value of whichever site is being converted (`archive_dest` at `:164` and `:216`, `md_destination` at `:220`, `destination` at `:258`). The `from: "(converted)"` entry at `:220` keeps its literal sentinel — redesigning it is explicitly out of scope.

- Delete the end-of-run block at `src/organiser.rs:274-278`:

```rust
    // Write undo log
    let log_path = dir.join(LOG_FILE);
    let log_contents =
        serde_json::to_string_pretty(&log).map_err(|e| MaidError::UndoFailed(e.to_string()))?;
    fs::write(log_path, log_contents)?;
```

4. Verify:

```
cargo nextest run -p maid -- journal    → 5 passed
cargo nextest run -p maid              → all green
cargo clippy -p maid -- -D warnings   → zero warnings
```

5. `git branch --show-current` → `fix/collision-safe-destinations`.
   Commit: `git commit -m "fix(organiser): persist undo journal per completed move"`

---

## Task 6: Point `undo` at `read_log`

**Crate**: `maid`
**File(s)**: `src/organiser.rs`
**Run**: `cargo nextest run -p maid -- undo`

Split out from Task 5 so the journal is testable before `undo`'s behaviour changes.

1. In `undo`, replace the read block at `src/organiser.rs:290-300`:

```rust
    let (log_path, log) = read_log(dir)?;
```

and delete the now-unused `let log_path = dir.join(LOG_FILE);` and the `serde_json::from_str` line. `log_path` is reused at the end for `fs::remove_file(&log_path)?`, which is what makes the legacy file get cleaned up after a successful undo.

2. Verify:

```
cargo nextest run -p maid    → all green
cargo clippy -p maid -- -D warnings   → zero warnings
```

3. Confirm no live-log regression by hand — the three real logs must still read:

```
./target/debug/maid undo ~/Downloads    # dry-check only; do NOT complete
```

Safer verification, no mutation:

```bash
cargo build
./target/debug/maid --help >/dev/null   # sanity
ls -l ~/Documents/.maid_log.json ~/Desktop/.maid_log.json ~/Downloads/.maid_log.json
```

The legacy path is covered by `read_log_falls_back_to_the_legacy_array`; do not exercise a real undo against the user's three live logs without asking.

4. `git branch --show-current` → `fix/collision-safe-destinations`.
   Commit: `git commit -m "refactor(organiser): read undo journal through legacy-aware fallback"`

---

## Task 7: Guard `undo`'s restore against clobbering

**Crate**: `maid`
**File(s)**: `src/organiser.rs`
**Run**: `cargo nextest run -p maid -- restore`

1. Write the failing tests:

```rust
    #[test]
    fn restore_entry_refuses_to_overwrite_existing_file() {
        let dir = temp_dir("restore-guard");
        let to = dir.join("inbox.md");
        let from = dir.join("original.md");
        fs::write(&to, b"moved\n").expect("seed to");
        fs::write(&from, b"user wrote this after the run\n").expect("seed from");

        let result = restore_entry(&from, &to);
        assert!(matches!(result, Err(MaidError::UndoFailed(_))));
        assert_eq!(
            fs::read_to_string(&from).expect("read from"),
            "user wrote this after the run\n"
        );
        cleanup(&dir);
    }

    #[test]
    fn restore_entry_moves_when_destination_is_free() {
        let dir = temp_dir("restore-ok");
        let to = dir.join("inbox.md");
        let from = dir.join("original.md");
        fs::write(&to, b"moved\n").expect("seed to");

        restore_entry(&from, &to).expect("restores");
        assert!(!to.exists(), "source must be gone");
        assert_eq!(fs::read_to_string(&from).expect("read from"), "moved\n");
        cleanup(&dir);
    }
```

2. Run `cargo nextest run -p maid -- restore` → expected **FAIL** (`cannot find function restore_entry`)

3. Implement:

```rust
/// Restores `to` to `from` for one log entry.
///
/// Returns `MaidError::UndoFailed` if a file already exists at `from`;
/// never overwrites user data. Unlike `resolve_destination` this does not
/// suffix: the original filename recorded in the journal is authoritative.
fn restore_entry(from: &Path, to: &Path) -> Result<(), MaidError> {
    if from.exists() {
        return Err(MaidError::UndoFailed(format!(
            "cannot restore: {} already exists",
            from.display()
        )));
    }
    fs::rename(to, from)?;
    Ok(())
}
```

4. Replace the restore branch in `undo` at `src/organiser.rs:306-309`:

```rust
        } else {
            restore_entry(Path::new(&entry.from), Path::new(&entry.to))?;
            println!(" Restored: {}", entry.from);
        }
```

5. Verify:

```
cargo nextest run -p maid -- restore    → 2 passed
cargo nextest run -p maid              → all green
cargo clippy -p maid -- -D warnings   → zero warnings
```

Note the behaviour change: a restore collision now aborts the replay mid-way, leaving undo partially applied. This is intended — a loud partial state beats a silent overwrite — and belongs in the README.

6. `git branch --show-current` → `fix/collision-safe-destinations`.
   Commit: `git commit -m "fix(organiser): refuse to overwrite on undo restore"`

---

## Task 8: Show full resolved paths in `preview`

**Crate**: `maid`
**File(s)**: `src/organiser.rs`, `src/main.rs`
**Run**: `cargo build`

1. Change the signature at `src/organiser.rs:60`:

```rust
pub fn preview(entries: &[FileEntry], dir: &Path, config: &Config) -> Result<(), MaidError> {
```

and terminate the function with `Ok(())` in place of the implicit unit return.

2. Update the move branch at `src/organiser.rs:111-113`:

```rust
        } else {
            let dest = config.destination(&entry.folder, dir);
            let resolved = resolve_destination(dest.join(entry.path.file_name().unwrap_or_default()))?;
            println!(" {} -> {}", filename, resolved.display());
        }
```

3. Amend the convert branch message at `src/organiser.rs:100-105` so preview does not over-promise — the quarantine verdict needs a file that does not exist yet:

```rust
            println!(
                " {} -> CONVERT ({}) -> {} + archive original; secret scan may divert it",
                filename, tool, md_name
            );
```

4. Add the `?` at the call site, `src/main.rs:73`:

```rust
                organiser::preview(&entries, &dir, &config)?;
```

5. Verify:

```
cargo nextest run -p maid              → all green
cargo clippy -p maid -- -D warnings   → zero warnings
cargo build                           → clean
```

6. Manual check, no mutation:

```bash
mkdir /tmp/maidprev; touch /tmp/maidprev/a.md /tmp/maidprev/b.md
./target/debug/maid preview /tmp/maidprev
rm -r /tmp/maidprev
```

Expected: each line now ends in a full filename, e.g. `a.md -> /path/to/00_Inbox/a.md`, not a bare directory.

7. `git branch --show-current` → `fix/collision-safe-destinations`.
   Commit: `git commit -m "feat(organiser): print resolved final paths in preview"`

---

## Task 9: Document the change

**Crate**: `maid`
**File(s)**: `README.md`
**Run**: `cargo fmt --all`

1. Add a "Collision handling" subsection under "How it works":

```markdown
### Collision handling

If a file already exists at its destination, maid never overwrites it. It
inserts a `-N` suffix before the extension and increments until it finds a
free name, so `invoice.md` becomes `invoice-1.md`, then `invoice-2.md`. This
applies to every destination: moved files, quarantined files, and both
archive paths. `preview` shows the resolved name, so you always see the
filename that will land on disk.

A run that cannot find a free name after 1000 attempts fails with an error
rather than overwriting.
```

2. Update the "Log file" subsection:

```markdown
### Log file

`maid run` writes `.maid_log.jsonl` into each target directory, appending one
line per completed move as each one finishes. Because the record is written
per move rather than at the end, a run that fails partway still leaves a
replayable record of everything that already happened.

`maid undo` reads that journal, reverses each entry, deletes the folders it
created, and removes the journal. If a journal in the older
`.maid_log.json` array format is found, it is read for backwards
compatibility and removed after a successful undo.

Restoring never overwrites: if something already occupies a file's original
location, `maid undo` reports the error and stops rather than replacing it.
```

3. Note the known limits honestly:

```markdown
> **Not atomic.** Checking for a free name and then renaming is two steps, so
> two concurrent `maid run` processes could pick the same name. Only the most
> recent run in a directory is undoable — a later run replaces the journal.
```

4. Verify `cargo fmt --all` produces no diff, and that no hook or cron job
   parses `maid preview` output (the format changed in this release).

5. `git branch --show-current` → `fix/collision-safe-destinations`.
   Commit: `git commit -m "docs(readme): document collision handling and journal format"`

---

## Final Verification

```bash
cargo fmt --all
cargo clippy --workspace -- -D warnings
cargo nextest run --workspace
git branch --show-current    # fix/collision-safe-destinations
git status --short           # expect only docs/ and .ctx/ untracked
```

Test count must rise from 33 to 49: +2 error Display (T1), +6 resolver (T3), +1 collision regression (T4), +5 journal (T5), +2 restore (T7).

## Pre-Save Checklist

- [x] Every design-doc requirement maps to a task (§1 resolver → T3, write sites → T4, §2 journal → T5/T6, §3 restore guard → T7, §4 preview → T8, README → T9)
- [x] No placeholders. All code blocks are copy-paste ready.
- [x] Names consistent: `resolve_destination`, `append_log_entry`, `read_log`, `restore_entry`, `split_name`, `disambiguated`, `JOURNAL_FILE`, `LEGACY_LOG_FILE`, `MAX_DISAMBIGUATION_ATTEMPTS`, `MaidError::DestinationExhausted`
- [x] Every task is TDD and ends in a commit
- [x] Two deviations from the design doc, both flagged for review: the `#[cfg(test)]` config builder (T2) and the abort-on-collision undo behaviour (T7)
