# Contributing to Corten Desktop

Thank you for your interest in contributing to **Corten Desktop**! Corten is built in pure Rust with safety, modularity, and high-performance computing in mind. Whether you are reporting a hardware bug, implementing a Wayland protocol, or improving user interfaces, your contributions are warmly welcome.

---

## 🛠 Development Workflow

### 1. Workspace Layout
The Corten repository is organized as a Cargo workspace with distinct, modular crates:

| Crate | Responsibilities |
| :--- | :--- |
| `corten-compositor` | Core Smithay Wayland compositor, DRM/KMS backends, Glow OpenGL ES shaders, window layouts, tablet engine, input handling. |
| `corten-panel` | Acrylic status dock, StatusNotifierItem (SNI) tray, Polybar/Waybar-compatible JSON applet engine, audio/network widgets. |
| `corten-settings` | Iced-based visual configuration hub: multi-monitor topology canvas, input devices, night light, accessibility. |
| `corten-portal` | `xdg-desktop-portal-corten`: PipeWire screencast/screenshot portal and session inhibitor. |
| `corten-launcher` | Fuzzy search desktop application launcher and calculator. |
| `corten-files` | Dual-pane high-speed graphical file manager. |
| `corten-lock` & `corten-greeter` | PAM-authenticated session locker and display manager greeter. |
| `corten-polkit` | Modern authentication agent for PolicyKit privilege escalation. |
| `corten-ai` | Local AI HUD, 20-tool ReAct agent (`aura.rs`), HITL security gate (`hitl_audit.log`), and hybrid inference dispatcher. |
| `crates/vector` (`corten-vector`) | In-process HNSW-like vector database, JSONL Write-Ahead Log (`wal.rs`), metadata filtering, and LSH text embedder. |
| `crates/llm` (`corten-llm`) | Zero-copy `memmap2` GGUF parser and CPU/Rayon quantized tensor runner (`Q4_K`, `Q6_K`, `Q5_0`, `Q8_0`, `F16`). |
| `crates/services` (`corten-services`) | DRM HDR10 metadata serializer, CIE 1931 / ST.2084 PQ color management, ICC parser, and AT-SPI2 D-Bus tree. |
| `crates/ipc` (`cortenctl`) | Unix Domain Socket JSON-RPC protocol and command-line management utility. |
| `crates/config` | TOML deserialization and disk file watcher for hot-reloading configurations. |
| `crates/theme` | Design tokens, color palettes, and WCAG AAA contrast calculations. |

---

## 🧪 Testing & Code Quality

Before opening a pull request, ensure your code passes formatting, linter, and unit tests:

```bash
# 1. Format code
cargo fmt --all --check

# 2. Run Clippy (warnings treated as errors)
cargo clippy --workspace --all-targets -- -D warnings

# 3. Run all tests
cargo test --workspace
```

All 122+ unit and integration tests must pass cleanly.

---

## 🐛 Reporting Bugs

When filing an issue, please provide:
1. **GPU & Driver:** e.g., AMD Radeon RX 6700 XT (Mesa 24.1), NVIDIA RTX 3080 (Driver 555.58.02), Intel Iris Xe.
2. **Display Setup:** e.g., Dual monitor: 1440p@165Hz via DisplayPort + 1080p@60Hz via HDMI.
3. **Execution Mode:** Running nested under another compositor (GNOME/KDE/Sway) or running directly on bare-metal TTY (`corten-session`).
4. **Logs:** Run the compositor with debug output:
   ```bash
   RUST_LOG=corten_compositor=debug,smithay=info corten-compositor > corten.log 2>&1
   ```

---

## 💡 Submitting Pull Requests

1. Fork the repository and create a new feature branch:
   ```bash
   git checkout -b feature/my-cool-feature
   ```
2. Write clean, idiomatic Rust. Prefer zero-allocation paths in rendering loops.
3. Add unit tests for newly introduced logic (layout calculations, IPC commands, state mutations).
4. Keep commit messages clear and descriptive:
   - `compositor: add support for ext-workspace-v1 protocol`
   - `panel: fix tray icon pixmap alignment in statusnotifieritem`
   - `settings: add slider for touchpad acceleration speed`
5. Submit your PR against the `main` branch.

