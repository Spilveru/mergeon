# Contributing to Mergeon

Thank you for your interest in contributing to **Mergeon**!

## Prerequisites

- **Rust** stable toolchain (Edition 2024, pinned in [`native/rust-toolchain.toml`](native/rust-toolchain.toml))
- **Git** installed and available in `PATH`

### Linux (Debian / Ubuntu)

```bash
sudo apt-get update && sudo apt-get install -y \
  build-essential clang cmake pkg-config git python3 \
  libasound2-dev libfontconfig-dev libfreetype-dev libssl-dev \
  libvulkan-dev libwayland-dev libx11-xcb-dev libxcb1-dev \
  libxkbcommon-dev libxkbcommon-x11-dev libzstd-dev
```

For headless UI tests on Linux, also install:

```bash
sudo apt-get install -y xvfb openbox mesa-vulkan-drivers
```

### Windows

- Visual Studio Build Tools (MSVC C++ workload)
- Optional: `lld-link` (LLVM) for faster linking during tests
- Optional: NSIS 3.x for building the Windows installer

### macOS

- Xcode Command Line Tools (with Metal Toolchain for GPUI shaders)

## Development Workflow

All Rust source code lives in [`native/`](native/).

```bash
cd native

# Run the application
cargo run -p mergeon

# Run formatting and lint checks (CI enforces -D warnings)
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings

# Run the full test suite (use tools/in-x.sh on headless Linux)
cargo test --workspace --locked
# or on headless Linux:
tools/in-x.sh cargo test --workspace --locked
```

> Tip: `[profile.dev.package."*"]` compiles dependencies with `opt-level = 2` so GPUI renders
> smoothly in debug builds while keeping incremental rebuilds of Mergeon crates fast.

## Project Structure

- `native/crates/mergeon-gitcmd` — Git process runner, cancellation, timeouts, credentials
- `native/crates/mergeon-git-query` — Read-only Git queries (log, status, diff, blame, file history)
- `native/crates/mergeon-git-ops` — Mutating Git operations (stage, commit, rebase, merge, undo)
- `native/crates/mergeon-runtime` — Per-repo mutation queue and filesystem watcher
- `native/crates/mergeon-store` — Atomic settings persistence and token encryption interface
- `native/crates/mergeon-github` — GitHub OAuth device flow, repos, and pull requests
- `native/crates/mergeon-updater` — HTTPS + Ed25519-verified auto-updater
- `native/crates/mergeon-backend` — Synchronous backend facade + OS keyring cipher
- `native/crates/mergeon-model` — Pure domain models (commit graph, diff parsing, tabs, shortcuts)
- `native/crates/mergeon-app` — GPUI application state (`AppModel`) and async flows
- `native/crates/mergeon-ui` — Design system, theme, fonts, icons, and reusable widgets
- `native/crates/mergeon-view-*` — Individual views (`home`, `sidebar`, `toolbar`, `commits`, `changes`, `diff`)
- `native/crates/mergeon` — Desktop binary, window shell, command palette, and global shortcuts

## Pull Request Guidelines

1. Keep pull requests focused on a single feature, fix, or improvement.
2. Add or update unit/integration tests for your changes.
3. Make sure `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace` pass.
4. For security-sensitive bugs, follow [`SECURITY.md`](SECURITY.md) instead of opening a public PR or issue.

## Licensing

By contributing to Mergeon, you agree that your contributions will be licensed under the project's
license (**GNU General Public License v3.0 or later**, `GPL-3.0-or-later`), except for
modifications inside `native/vendor/gpui-pre/`, which are licensed under **Apache-2.0** to stay
compatible with upstream Zed/GPUI.
