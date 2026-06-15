# neatly

A clean, fast CLI tool that organises files in a directory by sorting them into subfolders based on their file type. Built with Rust.

```
Downloads/
├── invoice.pdf
├── photo.jpg
├── notes.docx
└── script.py
        ↓  neatly run
Downloads/
├── documents/
│   ├── invoice.pdf
│   └── notes.docx
├── images/
│   └── photo.jpg
└── code/
    └── script.py
```

---

## Features

- **Preview** — see exactly what would happen before any files are moved
- **Organise** — sort files into subfolders by type with a single command
- **Undo** — restore everything back to its original location and remove the created folders
- **Safe** — hidden files and subfolders are never touched

---

## File Categories

| Folder       | Extensions                      |
| ------------ | ------------------------------- |
| `images/`    | jpg, jpeg, png, gif, svg, webp  |
| `documents/` | pdf, docx, doc, txt, xlsx, pptx |
| `video/`     | mp4, mov, avi, mkv              |
| `audio/`     | mp3, wav, flac, aac             |
| `code/`      | rs, py, js, ts, html, css, json |
| `archives/`  | zip, tar, gz, rar               |
| `unknown/`   | everything else                 |

---

## Installation

### Prerequisites

- [Rust](https://www.rust-lang.org/tools/install) (1.70 or higher)

### Build from source

```bash
# Clone the repository
git clone https://github.com/mercy-dapps/neatly.git
cd neatly

# Build release binary
cargo build --release

# Install globally (macOS/Linux)
cp target/release/neatly /usr/local/bin/
```

After installation, `neatly` is available from anywhere in your terminal.

---

## Usage

### Preview

See what would happen without moving any files:

```bash
neatly preview ~/Downloads
```

Example output:

```
Preview - no files will be moved:

 invoice.pdf -> documents/
 photo.jpg -> images/
 song.mp3 -> audio/
 script.py -> code/

4 file(s) would be moved.
```

### Organise

Sort files into subfolders:

```bash
neatly run ~/Downloads
```

Example output:

```
 invoice.pdf -> documents/
 photo.jpg -> images/
 song.mp3 -> audio/
 script.py -> code/

4 file(s) organised.
```

### Undo

Restore all files to their original location and remove the created folders:

```bash
neatly undo ~/Downloads
```

Example output:

```
 Restored: /Users/mac/Downloads/invoice.pdf
 Restored: /Users/mac/Downloads/photo.jpg
 Restored: /Users/mac/Downloads/song.mp3
 Restored: /Users/mac/Downloads/script.py

4 file(s) restored.
```

---

## Recommended Workflow

Always run `preview` before `run` to confirm the output looks right:

```bash
# 1. Check what will happen
neatly preview ~/Desktop

# 2. Organise
neatly run ~/Desktop

# 3. Made a mistake? Undo it
neatly undo ~/Desktop
```

---

## How it works

- `neatly run` moves files and writes a `.neatly_log.json` file to the target directory, recording every move
- `neatly undo` reads the log, reverses every move, deletes empty folders, and removes the log file
- Hidden files (starting with `.`) are always skipped
- Subdirectories are never touched

---

## Built with

- [clap](https://github.com/clap-rs/clap) — CLI argument parsing
- [serde](https://serde.rs) + [serde_json](https://github.com/serde-rs/json) — undo log serialisation

---

## License

MIT
