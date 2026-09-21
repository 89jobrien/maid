# Maid — Agent Operating Guide

Maid is a file organizer CLI that sorts files into subfolders by type, converts
documents to markdown, and routes them to Obsidian vaults. This guide documents
build commands, code style, testing patterns, and development workflows.

## Build, Lint, and Test Commands

### Quick Reference

```bash
# Build and install
cargo build                   # Debug build
cargo build --release        # Release build
cargo install --path .       # Install to ~/.cargo/bin/maid

# Run tests
cargo test                    # Run all tests
cargo test test_name          # Run specific test

# Format and lint
cargo fmt                     # Auto-format code
cargo fmt --check            # Check formatting without changing
cargo clippy -- -D warnings   # Lint with strict mode

# CI check (mirrors GitHub Actions)
cargo fmt --check && \
  cargo clippy --all-targets -- -D warnings && \
  cargo test
```

### Running Tests

```bash
# Unit tests (in-source #[cfg(test)] modules)
cargo test

# Specific test function
cargo test test_config_load

# Tests matching pattern
cargo test config_

# With output (fails silently by default)
cargo test -- --nocapture

# Run nextest (faster)
cargo nextest run
```

## Code Style Guidelines

### Rust Version & Toolchain

- **Rust Version**: Latest stable (no pinned version)
- **Edition**: 2024
- **Components**: rustfmt, clippy

### Formatting (rustfmt)

```toml
# From rustfmt.toml
max_width = 100
edition = "2024"
use_small_heuristics = "Default"
```

- Line width: 100 characters maximum
- Consistency matters more than personal preferences

### Linting (clippy)

- **Strict linting**: `cargo clippy --all-targets -- -D warnings`
- No hard disallowed methods (unlike maestro)
- Avoid `unwrap()` and `expect()` in production code

### Naming Conventions

- **Structs/Enums**: PascalCase (`MaidConfig`, `FileType`)
- **Functions/Methods/Variables**: snake_case (`organize_files`, `file_type`)
- **Constants**: SCREAMING_SNAKE_CASE (`DEFAULT_CONFIG_PATH`)
- **Modules**: snake_case (`config`, `organiser`)
- **Files**: snake_case.rs (`config.rs`, `organiser.rs`)

### Imports & Dependencies

```rust
// Standard pattern - group by crate, then std
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
```

### Error Handling

- **Primary**: `MaidError` enum (application-specific errors)
- **Propagation**: Use `?` operator
- **User-facing**: Return `MaidError` from CLI handlers
- **Logging**: Use `eprintln!` for errors

### Code Structure Patterns

#### CLI Applications (clap)

```rust
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "maid")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Organize files
    Organize {
        #[arg(value_name = "PATH")]
        path: PathBuf,
    },
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Organize { path } => organize::run(&path)?,
    }
    Ok(())
}
```

#### Configuration (serde + toml)

```rust
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
struct MaidConfig {
    vault_path: String,
    categories: Vec<Category>,
}

impl MaidConfig {
    fn load_from_file(path: &Path) -> Result<Self, MaidError> {
        let content = std::fs::read_to_string(path)?;
        toml::from_str(&content).map_err(Into::into)
    }
}
```

#### File Operations

```rust
use std::fs;
use std::path::Path;

// Safe file operations with error handling
fn copy_with_metadata(src: &Path, dst: &Path) -> Result<(), MaidError> {
    fs::copy(src, dst)?;
    Ok(())
}

fn rename_file(src: &Path, dst: &Path) -> Result<(), MaidError> {
    fs::rename(src, dst).map_err(Into::into)
}
```

## Testing Patterns

### Test Organization

- **Unit tests**: In same file as implementation (`#[cfg(test)] mod tests {}`)
- **Integration tests**: In `tests/` directory as separate files

### Common Test Patterns

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_loading() {
        // Setup
        let config = MaidConfig::default();

        // Assert
        assert!(!config.categories.is_empty());
    }

    #[test]
    fn test_file_organization() {
        let src = PathBuf::from("/tmp/test.pdf");
        let result = organize_file(&src);
        assert!(result.is_ok());
    }
}
```

### Test Isolation

- Use `std::fs::create_dir_tempfile` or similar for temp dirs
- Clean up after tests
- Avoid hardcoded paths

## Project Structure

### Layout

```text
maid/
├── src/
│   ├── main.rs        # CLI entry, command dispatch
│   ├── config.rs      # Configuration loading, categories
│   ├── organiser.rs   # Core logic: scan, preview, organize, convert
│   └── error.rs       # MaidError enum
├── tests/             # Integration tests
├── Cargo.toml
└── rustfmt.toml
```

### Module Responsibilities

- **`main.rs`** — CLI parsing (clap), command dispatch, error formatting
- **`config.rs`** — Load `~/.config/maid/config.toml`, category classification,
  destination resolution
- **`organiser.rs`** — Scan directories, preview organization, execute moves,
  handle conversions, obfsck gating
- **`error.rs`** — `MaidError` enum with Display impl for CLI output

## Development Workflow

1. **Setup**: `cargo build` to verify dependencies
2. **Development**: Edit source, run `cargo clippy` to lint
3. **Testing**: `cargo test` frequently
4. **Pre-commit**: Run `cargo fmt --check && cargo clippy -- -D warnings`
5. **Install**: `cargo install --path .`

## Key Dependencies

- **CLI**: `clap` with derive feature
- **Config**: `serde` with `derive`, `toml` for TOML parsing
- **Path utilities**: `dirs` for platform-specific paths
- **Time**: `chrono` for timestamps
- **Serialization**: `serde_json` for JSON output

## Commit Guidelines

### Pre-commit Checks

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

### Commit Message Style

Follow [Conventional Commits](https://www.conventionalcommits.org/):

Format: `<type>(<scope>): <description>`

Examples:

- `feat(config): add custom category mapping`
- `fix(organiser): handle symlinks correctly`
- `docs: update config file format`

Types:

- `feat:` New features
- `fix:` Bug fixes
- `docs:` Documentation
- `refactor:` Code restructuring
- `test:` Testing changes
- `chore:` Maintenance
