# Design: Collision-Safe Destination Resolution and Durable Undo Logging

## Context Map

Produced from `614826e` on `fix/obfsck-secret-detection`.

### Files to Modify

| File               | Purpose                          | Changes Needed                                                                                                                                                          |
| ------------------ | -------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `src/organiser.rs` | scan / preview / organise / undo | Add resolver, add journal read+append, rewire 4 write sites, rework `undo` read path and restore guard, update `preview` output, add module consts, add in-source tests |
| `src/error.rs`     | `MaidError` enum                 | Add `DestinationExhausted` variant + `Display` arm (the match at `:16-19` is exhaustive, so this is mandatory)                                                          |
| `README.md`        | User documentation               | Document collision behaviour, journal file name and format, legacy fallback                                                                                             |

### Dependencies (may need updates)

| Reference            | Location                                                         | Relationship                                                                                         |
| -------------------- | ---------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------- |
| `LOG_FILE` const     | `organiser.rs:26`, used at `:275`, `:290`                        | Split into `JOURNAL_FILE` + `LEGACY_LOG_FILE`; both call sites change                                |
| `LogEntry` struct    | `organiser.rs:21`                                                | **Shape unchanged.** JSONL is a framing change, not a schema change — one serialised object per line |
| `MaidError` variants | constructed at `organiser.rs:35,138,277,293,300`, `config.rs:57` | New variant is additive; no existing construction site changes                                       |

**No external consumers.** This is a binary-only crate with no `lib.rs`, so there is no public API surface and no semver exposure. Every item above is internal to the crate.

### Test Coverage

| Test location            | Covers                                                                                                                                                              |
| ------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `organiser.rs` in-source | 14 tests: `swap_ext`, `is_notes_ext`, `LogEntry` JSON round-trip, `scan` rejects missing dir, `scan` skips hidden/subdirs, `file_age_days`, 6 × `audit_match_count` |
| `config.rs` in-source    | 19 tests: classify / destination / action / stale / converter / expand_path / quarantine+archive                                                                    |

**Coverage gaps this design must close** — no existing test exercises `organise`, `undo`, or any write site. The three required new tests are specified under Public API → Tests.

### Reference Patterns

| Pattern                              | Existing example                               | Follow                                    |
| ------------------------------------ | ---------------------------------------------- | ----------------------------------------- |
| Serde struct for a persisted record  | `LogEntry` at `organiser.rs:20-23`             | Reuse as-is; do not redesign              |
| Error variant carrying a `String`    | `InvalidDirectory(String)` at `error.rs:7`     | New variant uses `String` for consistency |
| Module-level `SCREAMING_SNAKE` const | `LOG_FILE`, `NOTES_EXTENSIONS`, `SECS_PER_DAY` | New consts follow suit                    |
| Temp-dir filesystem test             | `scan_skips_hidden_files_and_subdirectories`   | Reuse for all new FS tests                |
| Non-recursive scan guard             | `organiser.rs:38-47`                           | Not modified by this design               |

### Risk

- [x] **Public API change: no.** Binary crate, no `lib.rs`, nothing external can depend on these items.
- [x] **Serialization format change: YES.** `.maid_log.json` (JSON array) → `.maid_log.jsonl` (one JSON object per line). Persisted on-disk state. **Three live logs exist today** (`~/Documents/.maid_log.json` 11.6 kB, `~/Desktop/.maid_log.json` 3.3 kB, `~/Downloads/.maid_log.json` 293 B, all ~3 months old). Mitigated by a mandatory legacy-read fallback. `LogEntry` itself is untouched.
- [x] **CLI output change: YES.** `preview` switches from `name -> destination_dir` to the full resolved final path. No consumers exist in this repo, but maid is a personal tool that may be invoked from hooks or cron; those would need re-checking.
- [x] **New external dependency: no.**
- [x] **Feature flag required: no.**
- [x] **Cross-crate boundary: no.** Single crate.

---

## Goal

Maid must never silently destroy a file, and every move it performs must remain reversible even when a run fails partway through.

## Approved Approach

Collision-safe destination resolution by appending a `-N` suffix before the extension, applied at all four destination write sites, combined with per-entry append-only JSONL undo logging that falls back to reading the legacy JSON array.

## Crate Ownership

- **Owner crate**: `maid` — single-crate binary; the change is internal to `organiser.rs` with one additive error variant. No new crate: the resolver and journal are ~40 lines total and a separate crate would be unjustified ceremony.
- **Affected crates**: none.

## Public API

**There is no public API.** `maid` is a binary-only crate (`Cargo.toml` declares no `[lib]`, no `lib.rs` exists). Everything below is crate-internal. This is recorded explicitly because the design skill's default is to enumerate public surface — here the answer is that there is none, which is a risk reduction rather than an omission.

### Types

No new types. `LogEntry` is unchanged:

```rust
pub struct LogEntry {
    pub from: String,
    pub to: String,
}
```

JSONL changes only the _framing_ (one serialised object per line) and not the schema, so the legacy array and the new journal deserialise through the same type.

### Error Variants

```rust
pub enum MaidError {
    DestinationExhausted(String),
}
```

`String` for consistency with `InvalidDirectory` / `UndoFailed` / `ConfigError`. Message content: the desired filename and the directory in which no free name was found.

### Constants

```rust
const JOURNAL_FILE: &str = ".maid_log.jsonl";
const LEGACY_LOG_FILE: &str = ".maid_log.json";
const MAX_DISAMBIGUATION_ATTEMPTS: u32 = 1000;
```

`LEGACY_LOG_FILE` carries the value currently held by `LOG_FILE`; `LOG_FILE` is removed.

### Functions

```rust
/// Resolves `desired` to a path that does not exist by inserting a `-N`
/// suffix before the extension, incrementing from 1.
///
/// Fast path: returns `desired` unchanged when that path is already free,
/// so non-colliding files keep byte-identical destinations.
///
/// Returns `MaidError::DestinationExhausted` when
/// `MAX_DISAMBIGUATION_ATTEMPTS` candidates are exhausted. Never falls back
/// to overwriting.
fn resolve_destination(desired: PathBuf) -> Result<PathBuf, MaidError>;

/// Appends one `LogEntry` as a single JSON line, creating the journal if
/// absent. Called immediately after each successful move.
fn append_log_entry(dir: &Path, entry: &LogEntry) -> Result<(), MaidError>;

/// Reads the undo journal for `dir`, preferring `JOURNAL_FILE` and falling
/// back to the legacy `LEGACY_LOG_FILE` array. Unparseable trailing lines are
/// skipped so a torn final append does not lose earlier entries.
///
/// Returns the file it actually read so the caller removes the right one.
fn read_log(dir: &Path) -> Result<(PathBuf, Vec<LogEntry>), MaidError>;
```

`LogEntry` requires no change, so no other signature in the crate is altered.

### Write Sites Converted

All four destination writes, currently bare `fs::rename`:

| Line   | Site                        | Desired path built from                             |
| ------ | --------------------------- | --------------------------------------------------- |
| `:169` | stale-note archive          | `archive_dir.join(format!("{}-{}", now, filename))` |
| `:205` | converted-output move       | `md_dest_dir.join(&md_name)`                        |
| `:214` | converted-original archive  | `archive_dir.join(format!("{}-{}", now, filename))` |
| `:263` | primary move and quarantine | `dest_dir.join(filename_os)`                        |

**Explicitly excluded:** `organiser.rs:502` inside `convert_with_marker` relocates `marker_single`'s own output into place. It is a tool-internal path, not a maid destination, and is not a collision candidate.

### Restore Guard

`organiser.rs:307` — `undo`'s `fs::rename(&entry.to, &entry.from)?` is a bare rename and will silently overwrite a file the user created at the original location after the run. The guard rule differs from the resolver's: restoring must **fail loudly**, never suffix, because the original filename is authoritative.

```rust
/// Restores `to` to `from` for one log entry.
///
/// Returns `MaidError::UndoFailed` if a file already exists at `from`;
/// never overwrites user data.
fn restore_entry(from: &Path, to: &Path) -> Result<(), MaidError>;
```

## Data Flow

1. **Source**: `scan` yields `FileEntry { path, folder }`, one level deep, hidden files and subdirectories excluded.
2. **Classify**: `Config::classify` maps extension → category; `Config::action_for` selects the action.
3. **Transform (action)**: note-and-stale / convert / notes-obfsck-gate / plain move, per the existing four-way resolution.
4. **Transform (resolve)**: the action builds a `desired` path; `resolve_destination` returns a collision-free equivalent.
5. **Sink**: `fs::rename` to the resolved path.
6. **Journal**: `append_log_entry` immediately after each successful sink.

Step 6 is per-entry, not per-run. That is the whole durability property: whatever `?` exits the loop — and there are 11 such sites at `:138, :158, :169, :190, :201, :205, :209, :214, :249, :253, :263` — the record of completed moves is already on disk.

**Ordering trade-off, stated explicitly.** The entry is appended _after_ the rename succeeds. If the append itself fails, the file is correctly at its destination but unrecorded. This is a strictly narrower window than today's whole-run window, and reversing the order is worse: a log entry for a move that never happened makes `undo` fail on a file that does not exist. The residual window is accepted and documented rather than engineered away.

## Hexagonal Boundaries

**None introduced. This is a deliberate, documented deviation from the skill's default.**

- `std::fs` is the platform, not an external dependency; a filesystem port trait would abstract nothing real.
- The crate currently contains zero traits. Adding one for a ~15-line path-probing function would triple the diff for no added test capability.
- Both new functions are testable against a real filesystem, following the existing precedent of `scan_skips_hidden_files_and_subdirectories` (`organiser.rs`).
- `MAX_DISAMBIGUATION_ATTEMPTS` exhaustion is tested by creating that many empty files in a temp dir, which is fast and keeps the bound honest.

## Tests

Three required, in-source, temp-dir based:

1. **Collision regression** — two source directories each holding `collision.md`, one shared `notes` destination. Assert both files survive with distinct resolved names, and that `undo` restores each to _its own_ origin. This is the exact scenario that destroyed data; it belongs in the suite permanently.
2. **Durability under mid-run failure** — drive `organise` with a destination that fails partway through, then assert the journal already contains every move completed before the failure.
3. **Resolver progression** — fast path returns the input unchanged; `invoice.md` → `invoice-1.md` → `invoice-2.md`; extensionless and pure-dotfile names behave as specified; exhaustion returns `DestinationExhausted` rather than overwriting.

Plus journal framing tests: JSONL round-trip, legacy array fallback, torn-trailing-line tolerance, and the restore guard refusing to overwrite.

Case-insensitivity is **not** unit-tested — it cannot be forced portably. `resolve_destination` relies on `Path::exists()` deferring to OS semantics, which is already correct on both case-insensitive APFS and case-sensitive Linux. This is a documented property of the target filesystem, and a future contributor must not "fix" it with a manual case-folding pass.

## Out of Scope

- **Multi-run undo history.** Per-entry persistence makes one run durable. A subsequent run still replaces the journal, so "only the most recent run is undoable" remains documented behaviour.
- **Full preview parity for conversions.** Whether converted markdown is quarantined depends on scanning a file that does not exist yet. Preview will show the resolved clean destination plus a note that the secret scan may divert it. Converting during a dry run is rejected as too costly and side-effecting.
- **Atomicity / concurrency.** Check-then-rename is not atomic; two concurrent runs could select the same disambiguated name. Closing it needs `renameat2(RENAME_NOREPLACE)`, which is neither portable nor stable in std. Documented, not closed.
- **The `(converted)` sentinel.** `from: "(converted)"` overloads a path field to mean "this entry is a delete". It cannot collide with a real path, so it works. Replacing it with a `Move`/`Delete` enum would change the on-disk format for a problem users have not hit.
- **`lib.rs` target / integration tests.** Not required; everything here is testable in-source. Remains a separate idea.
- **No config schema change.** The resolver requires no new keys; `config.toml` needs no edits.

## Risk

- [ ] Breaking API changes: **no** — binary crate, no public surface.
- [ ] New external dependency: **no.**
- [ ] Feature flag required: **no.**
- [x] Serialization format change: **yes** — `.maid_log.json` → `.maid_log.jsonl`. Three live logs on disk are preserved by the legacy fallback and are removed only after a successful undo of that legacy file.
- [x] CLI output change: **yes** — `preview` prints full resolved final paths. Verify no hook or cron job parses maid output.
- [x] Undo path is modified: **yes** — the restore guard changes failure behaviour. This is the path a user reaches _after_ something has already gone wrong, so it must be covered by the regression tests before the change lands, not after.
