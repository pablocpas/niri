# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Niri is a scrollable-tiling Wayland compositor written in Rust. It implements a unique window management paradigm where windows are arranged in columns on an infinite horizontal strip, with dynamic workspaces arranged vertically per monitor.

## Building and Testing

### Basic Commands

- **Build**: `cargo build --release`
- **Build without default features**: `cargo build --no-default-features`
- **Run**: `cargo run` (runs nested in a window for testing)
- **Run tests**: `cargo test --all --exclude niri-visual-tests`
- **Run with slow/randomized tests**:
  ```bash
  env RUN_SLOW_TESTS=1 PROPTEST_CASES=200000 PROPTEST_MAX_GLOBAL_REJECTS=200000 RUST_BACKTRACE=1 cargo test --release --all
  ```
- **Visual tests**: `cargo run -p niri-visual-tests` (requires GTK and libadwaita)
- **Validate config**: `niri validate`
- **Format code**: `cargo +nightly fmt --all`
- **Clippy**: `cargo clippy --all`

### Feature Flags

- Default features: `dbus`, `systemd`, `xdp-gnome-screencast`
- `profile-with-tracy`: Enable Tracy profiler (always-on mode)
- `profile-with-tracy-ondemand`: Enable Tracy profiler (on-demand mode, can run as main compositor)
- `profile-with-tracy-allocations`: Enable allocation profiling
- `dinit`: Enable dinit integration

### Running Local Builds

1. **Nested window** (primary testing method): `cargo run`
2. **TTY**: Switch to different TTY and run `cargo run`
3. **As main compositor**: Install normally, then overwrite binary with `sudo cp ./target/release/niri /usr/bin/niri`
4. **RPM package**: Use `cargo generate-rpm` on RPM-based distros

## Architecture

### Core Components

- **`src/niri.rs`**: Main compositor state (`Niri` struct) containing all Wayland protocol handlers, output management, and the event loop
- **`src/layout/`**: Window layout system implementing scrollable tiling
  - `mod.rs`: Top-level `Layout` managing all monitors
  - `monitor.rs`: Per-monitor state with workspaces
  - `workspace.rs`: Per-workspace scrolling layout logic
  - `scrolling.rs`: Column-based scrolling layout with tiles
  - `tile.rs`: Individual window tiles
  - `floating.rs`: Floating window management
  - `tests.rs`: Randomized property tests for layout operations
- **`src/backend/`**: Display backends
  - `tty.rs`: TTY/DRM backend for real hardware
  - `winit.rs`: Winit backend for nested testing
  - `headless.rs`: Headless backend
- **`src/handlers/`**: Wayland protocol handlers
  - `xdg_shell.rs`: XDG shell protocol (windows)
  - `layer_shell.rs`: Layer shell protocol (bars, backgrounds)
  - `compositor.rs`: Core compositor protocol
- **`src/input/`**: Input handling (keyboard, mouse, touchpad, gestures)
- **`src/protocols/`**: Additional Wayland protocols
- **`src/dbus/`**: D-Bus interfaces (accessibility, screencasting, etc.)
- **`src/render_helpers/`**: Rendering utilities
- **`src/animation/`**: Animation system with custom shader support

### Workspace Members

- **`niri-config`**: Configuration parsing using KDL format
- **`niri-ipc`**: IPC types for communication with running compositor
- **`niri-visual-tests`**: GTK app for visual testing of layout code

### Configuration

- Config files use KDL format (located at `~/.config/niri/config.kdl` or `/etc/niri/config.kdl`)
- Live-reloaded automatically on save
- Default config: `resources/default-config.kdl` (embedded in binary)
- Config parsing: implemented in `niri-config/` crate with `LayoutPart` pattern for includes

## Development Guidelines

### Design Principles

1. **Opening a new window must not affect existing window sizes**
2. **The focused window must not move on its own**
3. **Actions apply immediately** (animations don't delay input)
4. **Disabled eye-candy features don't impact performance**
5. **Be mindful of invisible state** (reduce surprise factor)

### Layout-Specific Rules

- Large windows/popups align top-left (most important content)
- Fixed pixel sizes affect window, proportional sizes affect tile (including borders)
- Fullscreen windows are part of scrolling layout, not a special layer

### Code Style

- Follow rustfmt config: `imports_granularity = "Module"`, `group_imports = "StdExternalCrate"`
- Use `error!` only for bugs (never unwrap, log and recover instead)
- Use `warn!` for user/hardware errors
- `info!` for important messages (shouldn't be overwhelming)
- `debug!` for less important messages
- `trace!` for debugging (compiled out in release)

### Testing

- When adding layout operations: add to `Op` enum in `src/layout/mod.rs` for randomized tests
- When adding config options: include in config parsing test in `niri-config/tests/`
- Client-server tests: `src/tests/` for complex Wayland interactions
- Visual tests: `niri-visual-tests` for visual inspection of layout/animations

### Profiling

Build with Tracy profiling:
```rust
pub fn some_function() {
    let _span = tracy_client::span!("some_function");
    // Function code
}
```

### IPC

- Socket at `$NIRI_SOCKET`
- `niri msg` is the CLI wrapper
- JSON protocol: write request on single line, read response as JSON
- Event stream available for real-time updates

## Common Tasks

### Adding a New Layout Operation

1. Add to `Op` enum in `src/layout/mod.rs` (auto-includes in randomized tests)
2. Add to `every_op` arrays if applicable
3. Implement the operation in relevant layout module
4. Add test cases if complex

### Adding a Config Option

1. Add to appropriate struct in `niri-config/src/`
2. Update parsing logic with KDL decoder
3. Include in `niri-config/tests/` parsing test
4. Update wiki documentation with examples and `Since:` annotation
5. Consider adding to `resources/default-config.kdl` if users should know about it

### Adding a Wayland Protocol

1. Add protocol XML to appropriate location
2. Generate bindings in `src/protocols/`
3. Implement handler in `src/handlers/` or new module
4. Register in `src/niri.rs`
5. Follow style of existing protocol implementations

### Pull Request Requirements

- Keep PRs focused on single feature/fix
- Split into small, self-contained commits (every commit should build and pass tests)
- Squash fixes from review into relevant commits
- Rebase to update main (don't merge)
- Run `cargo +nightly fmt --all` before pushing
- Test thoroughly including edge cases
- Document new config options on wiki
- Enable "Allow edits from maintainers"
