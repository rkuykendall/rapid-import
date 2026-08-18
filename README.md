# RapidImport

Photo/video auto-sort and import tool with multi-source capture-date
resolution, duplicate detection, and a reviewable dry-run plan before
anything touches disk.

RapidImport deliberately mirrors [RapidRAW](https://github.com/CyberTimon/RapidRAW)'s
architecture and conventions (Tauri 2 + Rust backend, React/TypeScript
frontend, the same `invoke`/`listen` command style, the same theme system
and UI components) so contributors familiar with one codebase feel at home
in the other. It's a separate project with no shared code or release
cycle — only the patterns are shared.

**Status: pre-alpha, local development only.** The core engine (scan, date
resolution, dedup, commit/undo) is solid and well-tested, and the UI now
covers the full loop — scan a source into a dry-run plan, review it, then
Import to actually move/copy files to the destination. No releases yet.

## Prerequisites

- [Rust](https://www.rust-lang.org/tools/install) 1.92 or newer
- [Node.js](https://nodejs.org/) 20 or newer
- Platform build tools for Tauri — see the
  [Tauri prerequisites guide](https://v2.tauri.app/start/prerequisites/)
  for your OS (Xcode Command Line Tools on macOS, WebView2 on Windows,
  `webkit2gtk` and friends on Linux)

## Running the app

```bash
npm install
npm start        # same as `tauri dev` — opens the app with hot reload
```

The SQLite index lives under the OS-standard app-data directory (e.g.
`~/Library/Application Support/com.rkuykendall.rapidimport/library.sqlite`
on macOS) — never inside a scanned photo library.

## Development

The Rust core lives entirely in `src-tauri/`, as a standalone library
crate (`rapid_import_core`) with no Tauri dependency of its own — the
scan/date-resolution/dedup/commit engine is fully testable in isolation.
`src-tauri/src/main.rs` is the thin Tauri layer wrapping it in commands.

```bash
cd src-tauri

cargo test                    # run the full test suite
cargo clippy --all-targets    # lint (kept warning-free)
```

There's also a standalone CLI for exercising the scan/plan engine without
the UI at all — useful for a quick dry-run against a real folder, or for
debugging the engine in isolation. Both binaries always read/write the same
`library.sqlite` the app itself uses (see above), so a reindex or dry run
from the CLI is visible to the app, and vice versa:

```bash
cargo run --bin scan_cli -- <source_dir> <destination_dir> [folder_template]
cargo run --bin reindex_cli -- <destination_dir> [scope_subfolder]
```

Frontend type-checking (not run automatically by `npm run build`, same as
RapidRAW — run it explicitly):

```bash
npx tsc --noEmit
```

## Building a release bundle

```bash
npm run tauri build
```
