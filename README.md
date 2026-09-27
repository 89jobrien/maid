# maid

A clean, fast CLI tool that organises files in a directory by sorting them into
subfolders based on their file type. Built with Rust.

```text
Downloads/
├── invoice.pdf
├── photo.jpg
├── notes.txt
└── script.py
        ↓  maid run
Downloads/
├── documents/
│   ├── invoice.pdf
│   └── notes.txt
├── images/
│   └── photo.jpg
└── code/
    └── script.py
```

---

## Features

- **Preview** — see exactly what would happen before any files are moved
- **Organise** — sort files into subfolders by type with a single command
- **Undo** — reverse the most recent run in a directory
- **Convert** — turn documents into markdown and archive the original
- **Quarantine** — route files that fail a secret scan out of the vault
- **Safe** — hidden files and subdirectories are never touched

---

## Installation

### Prerequisites

- [Rust](https://www.rust-lang.org/tools/install) (1.85 or higher — the crate
  uses edition 2024)

### Build from source

```bash
git clone https://github.com/89jobrien/maid.git
cd maid
cargo install --path .
```

After installation, `maid` is available from anywhere in your terminal.

### Optional external tools

Maid shells out to these when your config asks for them. All are optional and
detected at call time — if one is missing, the affected step is skipped and a
warning is printed.

| Tool            | Used for                                   |
| --------------- | ------------------------------------------ |
| `pandoc`        | Document → markdown conversion             |
| `marker_single` | OCR-heavy documents → markdown (PDF/image) |
| `mutool`        | Fast text extraction from PDFs             |
| `obfsck`        | Secret scan gating `.md`/`.mdx` and output |

### Secret scanning

`.md`/`.mdx` files and converted output are scanned with
`obfsck redact --audit` before being filed. Maid reads the total match count
from obfsck's audit report; anything above zero sends the file to quarantine
instead of its destination.

If no verdict can be obtained — `obfsck` is not installed, it exits non-zero, or
its report cannot be read — Maid prints a warning and treats the file as clean.
Failing closed in those cases would quarantine every note, so the gate errs
toward keeping files. Detection still honours your obfsck allowlist, level, and
profile, since it uses the same `redact` engine.

---

## Usage

Every command takes an optional path. With no path, Maid processes the
directories listed in your config, falling back to `~/Documents`, `~/Downloads`,
and `~/Desktop`.

### Preview

```bash
maid preview ~/Downloads
```

The output below assumes a config with `pdf` in `convert.mutool` and `md` in
`categories.notes`; with stock defaults the same run just moves everything into
per-type subfolders.

```text
==> /Users/joe/Downloads

Preview - no files will be moved:

 invoice.pdf -> CONVERT (mutool) -> invoice.md + archive original
 photo.jpg -> /Users/joe/Downloads/images
 notes.md -> QUARANTINE (/Users/joe/.local/share/maid/quarantine)
 script.py -> /Users/joe/Downloads/code

4 file(s) would be processed, 0 noted in place.
```

An empty directory prints `Nothing to organise.`

### Organise

```bash
maid run ~/Downloads
```

```text
==> /Users/joe/Downloads
 mutool error: format error: cannot find version marker
 FAILED to convert: invoice.pdf
 photo.jpg -> /Users/joe/Downloads/images
 notes.md -> QUARANTINED (/Users/joe/.local/share/maid/quarantine)
 script.py -> /Users/joe/Downloads/code

3 moved, 0 converted, 1 quarantined, 0 noted, 0 stale archived.
```

### Undo

```bash
maid undo ~/Downloads
```

```text
==> /Users/joe/Downloads
 Restored: /Users/joe/Downloads/photo.jpg
 Restored: /Users/joe/Downloads/notes.md
 Restored: /Users/joe/Downloads/script.py
 Removed converted: /Users/joe/Downloads/notes/invoice.md

4 action(s) undone.
```

Note that undo **deletes** generated markdown rather than moving it back — the
original document was archived, not left in place.

Undo reads the log, replays every entry in order, then removes the folders Maid
created. Folders that are not empty are left alone.

**Only the most recent run in a given directory is undoable** — each `maid run`
overwrites that directory's log.

### Completions

```bash
maid completions > ~/.config/nushell/maid-completions.nu
```

Generates a Nushell completion script. The command works even if your config
file is missing or malformed.

---

## Recommended Workflow

Always run `preview` before `run` to confirm the output looks right:

```bash
maid preview ~/Desktop   # 1. check what will happen
maid run ~/Desktop       # 2. organise
maid undo ~/Desktop      # 3. made a mistake?
```

---

## How it works

`maid run` applies one action per file, in this order. The first match wins.

1. **Note** — if the category's action is `note` and the file is older than its
   `stale` threshold, the file is archived as `YYYYMMDD-<name>`. Otherwise it is
   left in place and counted as noted.
2. **Convert** — if the extension appears in a `convert` list, the file is
   converted to markdown, frontmatter is injected, the result is secret-scanned,
   and the original is archived. Clean markdown lands in the `notes` destination;
   dirty markdown is quarantined.
3. **Notes gate** — `.md` and `.mdx` files are secret-scanned. Failures are
   quarantined; passes get frontmatter injected and are moved normally.
4. **Move** — everything else is moved to its category destination.

A file's category is looked up from a flat extension-to-folder table built from
your `categories` config, and `unknown` is the fallback. Extension matching is
case-insensitive.

> **Do not list the same extension under two categories.** The table is keyed by
> extension, so a duplicate resolves to whichever category is inserted last —
> and category iteration order is not deterministic. Pick one.

### Safety

- Hidden files (names starting with `.`) are skipped
- Subdirectories are never entered or moved
- Scans are non-recursive — one level deep only

### Frontmatter

Maid prepends YAML frontmatter to markdown it moves or produces, unless the file
already begins with `---`:

```yaml
---
source: /Users/joe/Downloads/notes/invoice.md
moved_by: maid
moved_at: 2026-09-27T00:25:49Z
original_dir: Downloads
converted_from: /Users/joe/Downloads/invoice.pdf
---
<original content>
```

`converted_from` is only present for converted files.

### Log file

`maid run` writes `.maid_log.json` into each target directory, recording every
move and archive. `maid undo` reads it, reverses each entry, deletes the folders
it created, and removes the log.

---

## Configuration

Maid reads `~/.config/maid/config.toml`, falling back to the platform config
directory (`~/Library/Application Support/maid/config.toml` on macOS). If no
config file exists, the built-in defaults below apply.

```toml
# Directories used when no path is passed to a command.
# Empty or omitted -> ~/Documents, ~/Downloads, ~/Desktop
directories = ["~/Downloads", "~/Desktop"]

# Category name -> file extensions. Empty or omitted -> built-in defaults.
[categories]
images     = ["jpg", "jpeg", "png", "gif", "svg", "webp"]
documents  = ["pdf", "docx", "doc", "txt", "xlsx", "pptx"]
video      = ["mp4", "mov", "avi", "mkv"]
audio      = ["mp3", "wav", "flac", "aac"]
code       = ["rs", "py", "js", "ts", "html", "css", "json"]
archives   = ["zip", "tar", "gz", "rar"]
notes      = ["md", "mdx"]

# Category -> absolute destination. Omitted -> <source_dir>/<category>.
# `quarantine` and `archive` are also read from this table.
[destinations]
images     = "~/Pictures"
notes      = "~/Vault/Notes"
quarantine = "~/Vault/.quarantine"
archive    = "~/Documents/_RepoArchive/maid"

# Extensions routed through each converter. Omitted -> no conversion.
[convert]
pandoc = ["docx", "doc", "pptx", "xlsx"]
mutool = ["pdf"]
marker = ["pdf"]          # only used when marker_single is on PATH
fallback = "pandoc"        # used when marker is unavailable

# Category -> "move" (default) or "note".
[actions]
diagnostics = "note"

# Category -> age in days after which a "note" file is archived.
[stale]
diagnostics = 90
```

### Default categories

These apply only when `categories` is absent or empty.

| Folder       | Extensions                      |
| ------------ | ------------------------------- |
| `images/`    | jpg, jpeg, png, gif, svg, webp  |
| `documents/` | pdf, docx, doc, txt, xlsx, pptx |
| `video/`     | mp4, mov, avi, mkv              |
| `audio/`     | mp3, wav, flac, aac             |
| `code/`      | rs, py, js, ts, html, css, json |
| `archives/`  | zip, tar, gz, rar               |
| `unknown/`   | everything else                 |

Note that `md` and `mdx` are **not** in the defaults. Without a `notes` entry
in your config, markdown files classify as `unknown` and land in `unknown/` —
though they are still secret-scanned and get frontmatter either way.

### Default locations

| Purpose    | Default                               |
| ---------- | ------------------------------------- |
| Quarantine | platform data dir + `maid/quarantine` |
| Archive    | `~/Documents/_RepoArchive/maid`       |
| Config     | `~/.config/maid/config.toml`          |

The quarantine default resolves through the platform data directory — on macOS
that is `~/Library/Application Support/maid/quarantine`, on Linux
`~/.local/share/maid/quarantine`. Set `destinations.quarantine` to override it.

### Converter selection

`marker` entries are only used when `marker_single` is on `PATH`. Otherwise Maid
falls back to the `convert.fallback` tool, and if that is empty, skips
conversion for that file. `mutool` and `pandoc` entries are used directly.

### Path expansion

A leading `~/` in `directories` and `destinations` expands to your home
directory. Relative paths are used as-is.

---

## Built with

- [clap](https://github.com/clap-rs/clap) + `clap_complete` — CLI parsing and
  Nushell completion generation
- [serde](https://serde.rs) + [serde_json](https://github.com/serde-rs/json) —
  undo log serialisation
- [toml](https://github.com/toml-rs/toml) — config parsing
- [dirs](https://github.com/dirs-dev/dirs-rs) — platform config and data paths
- [chrono](https://github.com/chronotope/chrono) — timestamps in frontmatter
  and archive names

---

## License

MIT
