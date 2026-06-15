# maid

File organiser CLI — sorts files into subfolders by type, converts
documents to markdown, and routes them to Obsidian vaults.

## Build & Test

```bash
cargo build
cargo check
cargo clippy
cargo test
cargo install --path .
```

## Architecture

Single-binary CLI (clap). No lib crate.

- `main.rs` — CLI entry, loads config, dispatches subcommands
- `config.rs` — Config loading from `~/.config/maid/config.toml`, category
  classification, destination resolution, converter selection
- `organiser.rs` — scan, preview, organise, undo, conversion pipeline,
  obfsck gating, frontmatter injection
- `error.rs` — MaidError enum

## Config

`~/.config/maid/config.toml` — categories, destination overrides,
conversion tools, quarantine/archive paths.

## Key behaviors

- Documents (pdf, docx, etc.) are converted to markdown before filing
- Markdown files are checked by obfsck before moving to vault inboxes
- Files with detected secrets go to quarantine
- Originals of converted files are archived with date prefix
- Frontmatter injected into all markdown with provenance metadata
