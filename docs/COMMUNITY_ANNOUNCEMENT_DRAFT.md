# Corten Desktop — Technical Launch & Community Announcement Drafts

These drafts are written for systems engineers and developers on `r/rust`, `r/linux`, and **Hacker News (`Show HN`)**. They avoid marketing fluff and focus on verifiable code structure, protocol implementations, and explicit architectural trade-offs.

---

## 1. Reddit (`r/rust` / `r/linux`) Technical Post

**Title:** *Corten Desktop: A 68k-LOC modular Wayland desktop environment, compositor, and in-process GGUF/HNSW AI engine in pure Rust (Smithay + Iced)*

**Body:**

Hi everyone,

Over the past several months I’ve been engineering **Corten Desktop**, a standalone Wayland desktop environment written in Rust (`~68,700 LOC` across `130` `.rs` files, `28` workspace crates, and `122` unit tests).

Instead of wrapping `wlroots` or shipping a bare window manager that requires stitching together 15 external utilities, Corten compiles a complete DRM/KMS compositor, a 17-binary `wlr-layer-shell` / XDG desktop suite, an embedded HNSW vector engine, and a zero-copy GGUF inference runner from a single `Cargo.lock`.

### 1. Compositor Architecture (`corten-compositor` — 14,653 LOC)
Built on `Smithay`, supporting both direct DRM/KMS (`udev.rs` + `libseat`) and nested Winit development:
* **Layout Engine (`layout.rs`):** Supports Master-Stack, recursive Binary Space Partitioning (`BspNode::Split` with interactive drag-to-split when dropping a window into the inner 25% edge of another tile), floating windows with 14px edge/window magnetism (`snap_window_to_edges`), and an Exposé overview grid.
* **GLSL ES 3.0 Shader Pipeline (`glow_shader.rs`):** Implements multi-pass Dual-Kawase downsample/upsample background blur clipped via a signed-distance-field (`rounded_box`) corner mask (`GlassRegion`), preventing rectangular blur bleed behind rounded terminal/panel corners.
* **KMS Scanout & Sync (`udev.rs`, `handlers.rs`):**
  * **Explicit Sync (`linux-drm-syncobj-v1`):** Registers DRM timeline semaphores when `supports_syncobj_eventfd` is detected; attaches a pre-commit acquire blocker (`acquire_point.generate_blocker()`) and signals release points on scanout.
  * **Direct Scanout & Overlay Planes:** Uses `FrameFlags::ALLOW_PRIMARY_PLANE_SCANOUT_ANY | FrameFlags::ALLOW_OVERLAY_PLANE_SCANOUT` to bypass GL composition for fullscreen games and hardware video surfaces.
  * **Async Tearing (`wp_tearing_control_v1`) & VRR:** Bypasses VBlank wait loops when the focused fullscreen client requests `PresentationHint::Async`.
  * **DRM HDR10 Metadata (`crates/services/src/color_management.rs`):** Constructs 32-byte Linux kernel `hdr_output_metadata` blobs (`CEA-861-G` Type 1, SMPTE ST.2084 PQ EOTF, `0.00002` fixed-point CIE 1931 primaries) and binds them to connector `HDR_OUTPUT_METADATA` properties.
  * **Input (`tablet.rs`):** Full `zwp_tablet_manager_v2` stylus support (pressure, tilt X/Y, distance, eraser/pen tool descriptors) and touchpad kinetic momentum (`calculate_gesture_velocity` over a 150ms sliding window + damped spring physics).

### 2. Process-Isolated Shell (`corten-*` — Iced 0.13 + `tiny-skia`)
All desktop UI surfaces run as isolated OS processes communicating via `zbus` D-Bus interfaces and a Unix domain socket JSON-RPC bus (`corten-ipc` / `cortenctl`):
* **`corten-panel` (8,085 LOC):** Hosts `org.kde.StatusNotifierWatcher` (SNI system tray), a Waybar/Polybar-compatible JSON script applet runner (`{"text", "tooltip", "class", "percentage"}`), PipeWire audio VU-meter, and a mini process killer.
* **`corten-settings` (3,792 LOC):** Includes an interactive KScreen-style multi-monitor canvas (`offset_x`/`offset_y`), ICC profile header parser (`'acsp'` / `'mntr'`), and WCAG AAA accessibility controls backed by an AT-SPI2 D-Bus tree (`org.a11y.atspi.Accessible`).
* **`corten-files` (3,764 LOC), `corten-launcher` (6,064 LOC), `corten-portal` (795 LOC), `corten-lock`, `corten-greeter`, `corten-notify`, `corten-monitor`, `corten-snipper`.**

### 3. In-Process Local AI & Vector Engine (`corten-vector` + `corten-llm` + `corten-ai`)
A major goal of mine was integrating my standalone local AI project (**AURA-CORE**) directly into the desktop workspace with **zero Python/PyO3/FastAPI runtime dependencies**:
* **`crates/vector` (`corten-vector` — 691 LOC):** In-process `Arc<RwLock<InnerState>>` vector database with a navigable small-world graph (`build_hnsw_index`), parallel Rayon cosine/L2 search, inverted metadata index (`==`, `!=`, `>`, `<`, `>=`, `<=`), tombstone deletion + `vacuum()`, and JSON-lines Write-Ahead Logging (`wal.rs`). Linked directly into `corten-launcher` and `corten-ai` for `<0.2ms` semantic file lookup.
* **`crates/llm` (`corten-llm` — 1,961 LOC):** Zero-copy `memmap2::Mmap` GGUF file parser (`gguf.rs`), pure-Rust block dequantization kernels (`quant.rs`: `Q4_K`, `Q6_K`, `Q5_0`, `Q8_0`, `F16`), and Rayon-parallel `RMSNorm`, `RoPE`, `SwiGLU`, `KVCache`, and `MultiHeadAttention` (`ops.rs`, `engine/`).
* **`corten-ai/src/aura.rs` (3,619 LOC total in `corten-ai`):** Implements persistent user profile & vector memory (`~/.local/share/corten/aura/user_profile.json` + `memory_vector.wal`), a 20-tool native Rust registry (recursive-descent math parser, `/proc` telemetry, Git/file inspection, 9 `cortenctl` Wayland automation tools), a multi-step ReAct (`Thought -> Action -> Observation`) executor, and a Human-In-The-Loop (`HITL`) confirmation gate that intercepts privileged or destructive shell operations and records an audit trail (`hitl_audit.log`).

### 4. Engineering Trade-Offs & Current Limitations
* **CPU SIMD (`corten-llm`) vs. GPU Offload:** The embedded `corten-llm` crate runs on CPU via Rayon and is designed for zero-dependency execution of compact (`0.5B–3B`) GGUF models. When you need CUDA/ROCm GPU offloading or Vision (`llava`/`qwen2-vl`) screen analysis (`corten-snipper --fullscreen` -> RFC 4648 Base64), `corten-ai` automatically bridges to local `Ollama` (`11434`) or `llama-server` (`8080`) over loopback TCP.
* **Shell Rasterization (`tiny-skia`):** While `corten-compositor` renders via OpenGL ES/DRM, the layer-shell apps use Iced's `tiny-skia` software backend so 10+ background daemons don't allocate separate EGL/VRAM contexts (keeping total idle memory around `~180 MB`).

### Building & Running Tests
```bash
git clone https://github.com/Shura5860/corten-desktop.git
cd corten-desktop
cargo test --workspace
cargo clippy --workspace -- -D warnings
cargo build --release
```

Feedback, code audits, and multi-GPU / DRM hardware reports are very welcome!

---

## 2. Hacker News (`Show HN`) Submission

**Submit Page:** `https://news.ycombinator.com/submit`

**`title` (Max 80 chars):**
```text
Show HN: Corten – A 68k-LOC Wayland desktop and in-process GGUF/vector engine in Rust
```

**`url`:**
```text
https://github.com/Shura5860/corten-desktop
```

**`text` (veya gönderdikten sonra ilk yorum — Hacker News kalın/kod Markdown işaretlerini desteklemediği için HN'ye özel düz metin formatındadır):**
```text
Hi HN,

Corten is an independent Wayland desktop environment written in Rust (28 workspace crates, ~68.7k LOC, 123 unit tests). It combines a Smithay-based DRM/KMS compositor with an Iced shell suite and an embedded local AI/vector subsystem.

A few concrete implementation details for those interested in graphics, Wayland, or local inference:

1. Compositor & DRM (corten-compositor):
- Implements both Master-Stack and recursive BSP tree (BspNode) tiling with drag-to-split edge detection (layout.rs).
- Uses custom GLSL ES shaders (glow_shader.rs) for Dual-Kawase blur with an SDF rounded-rectangle mask (GlassRegion) so background blur never bleeds past rounded window corners.
- Supports linux-drm-syncobj-v1 (explicit sync timeline semaphores), wp_tearing_control_v1, zwp_tablet_manager_v2 (Wacom stylus pressure/tilt), and serializes 32-byte Linux hdr_output_metadata blobs (ST.2084 PQ / BT.2020) directly to DRM connectors (color_management.rs).

2. In-Process Vector & GGUF Engine (crates/vector, crates/llm, corten-ai):
- Rather than requiring a Python/FastAPI background server, we embedded a pure-Rust HNSW-like vector index (corten-vector) with JSON-lines Write-Ahead Logging (wal.rs), tombstone compaction, and metadata filtering (==, !=, >, <, >=, <=), plus a zero-copy memmap2 GGUF inference engine (corten-llm) supporting Q4_K, Q6_K, Q5_0, Q8_0, and F16 quantized blocks.
- corten-ai layers a 20-tool ReAct loop (aura.rs), persistent vector memory (user_profile.json + memory_vector.wal), and a Human-In-The-Loop (HITL) security gate (hitl_audit.log) on top, while also supporting loopback fallback to Ollama/llama.cpp when GPU VRAM offloading is needed.

3. Trade-Offs:
- Shell panels/applets (corten-panel, corten-settings, corten-files) use Iced's tiny-skia software rasterizer rather than per-process GL contexts to keep idle RAM at ~180 MB, while the compositor handles 60-240Hz GPU scanout.
- Built-in launcher file indexing uses deterministic 128-dim LSH character/word n-gram hashing (<0.2ms, zero model weights on disk) so Super+Space has zero cold-start latency.

Everything builds offline from a single workspace (cargo test --workspace). Happy to answer technical questions!
```
