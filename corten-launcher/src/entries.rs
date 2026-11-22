use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::{debug, info, warn};

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct AppEntry {
    pub id: String,
    pub name: String,
    pub generic_name: Option<String>,
    pub comment: Option<String>,
    pub exec: String,
    pub icon: Option<String>,
    pub icon_path: Option<PathBuf>,
    pub terminal: bool,
    pub categories: Vec<String>,
}

impl AppEntry {
    /// Launches the application by parsing the Exec line safely without invoking a shell.
    pub fn launch(&self) -> std::io::Result<()> {
        let args = parse_exec(&self.exec)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;

        // Support leading `env KEY=VAL ...` or `KEY=VAL ...` assignments without invoking `sh -c`
        let mut envs: Vec<(String, String)> = Vec::new();
        let mut idx = 0;
        if args.first().map(|s| s.as_str()) == Some("env") {
            idx += 1;
        }
        while idx < args.len() {
            if let Some((k, v)) = args[idx].split_once('=') {
                if !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                    envs.push((k.to_string(), v.to_string()));
                    idx += 1;
                    continue;
                }
            }
            break;
        }

        let (program, arguments) = args.get(idx..).and_then(|s| s.split_first()).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "empty Exec entry")
        })?;

        info!("Launching app '{}': {}", self.name, program);

        let wayland_display = std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "corten-0".into());
        let x11_display = std::env::var("DISPLAY").ok().or_else(|| {
            std::env::var("XDG_RUNTIME_DIR")
                .ok()
                .and_then(|d| {
                    std::fs::read_to_string(PathBuf::from(d).join("corten-x11-display")).ok()
                })
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .or_else(|| {
                    if std::path::Path::new("/tmp/.X11-unix/X0").exists() {
                        Some(":0".to_string())
                    } else {
                        None
                    }
                })
        });

        let mut cmd = Command::new(program);
        cmd.args(arguments)
            .env("WAYLAND_DISPLAY", &wayland_display)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        if let Some(ref disp) = x11_display {
            cmd.env("DISPLAY", disp);
        }
        for (k, v) in envs {
            cmd.env(k, v);
        }

        cmd.spawn().map(|_| ())
    }
}

/// Parse a desktop-entry Exec value without invoking a shell.
///
/// Desktop entry field codes are intentionally omitted because the launcher
/// has no file/URL selection context. `%%` is the only field code that has a
/// meaningful standalone expansion.
fn parse_exec(exec: &str) -> Result<Vec<String>, String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut escaped = false;

    for ch in exec.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }
        match ch {
            '\\' if quote != Some('\'') => escaped = true,
            '\'' | '"' if quote == Some(ch) => quote = None,
            '\'' | '"' if quote.is_none() => quote = Some(ch),
            c if c.is_whitespace() && quote.is_none() => {
                if !current.is_empty() {
                    args.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }

    if escaped || quote.is_some() {
        return Err("unterminated escape or quote in Exec entry".into());
    }
    if !current.is_empty() {
        args.push(current);
    }

    let mut expanded = Vec::with_capacity(args.len());
    for arg in args {
        if let Some(code) = arg.strip_prefix('%') {
            match code {
                "%" => expanded.push("%".into()),
                "f" | "F" | "u" | "U" | "d" | "D" | "n" | "N" | "i" | "c" | "k" | "v" | "m" => {
                    continue
                }
                _ => return Err(format!("unsupported desktop-entry field code: %{code}")),
            }
        } else {
            expanded.push(arg);
        }
    }
    Ok(expanded)
}

fn convert_svg_to_png(svg_path: &std::path::Path, out_size: u32) -> Option<PathBuf> {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let cache_dir = directories::BaseDirs::new()
        .map(|b| b.cache_dir().join("corten").join("icons"))
        .or_else(|| std::env::var("XDG_RUNTIME_DIR").ok().map(|d| PathBuf::from(d).join("corten_icon_cache")))
        .unwrap_or_else(|| PathBuf::from("/tmp/corten_icon_cache"));
    let _ = std::fs::create_dir_all(&cache_dir);

    let file_stem = svg_path.file_stem()?.to_string_lossy();
    let mut hasher = DefaultHasher::new();
    svg_path.hash(&mut hasher);
    let path_hash = hasher.finish();
    let out_file = cache_dir.join(format!("{}_{:08x}_{}x{}.png", file_stem, path_hash as u32, out_size, out_size));
    if out_file.exists() {
        return Some(out_file);
    }

    let status = std::process::Command::new("rsvg-convert")
        .args([
            "-w", &out_size.to_string(),
            "-h", &out_size.to_string(),
            svg_path.to_str()?,
            "-o", out_file.to_str()?,
        ])
        .status()
        .ok();

    if status.is_some_and(|s| s.success()) && out_file.exists() {
        return Some(out_file);
    }

    if rasterize_svg_pure_rust(svg_path, &out_file, out_size) {
        Some(out_file)
    } else {
        None
    }
}

/// Pure-Rust SVG icon fallback rasterizer when `rsvg-convert` is not installed:
/// extracts the primary hex fill/stroke color from the SVG XML and writes a valid
/// uncompressed/zlib PNG badge so SVG desktop icons still render without external binaries.
pub fn rasterize_svg_pure_rust(svg_path: &std::path::Path, out_file: &std::path::Path, out_size: u32) -> bool {
    let Ok(svg_text) = std::fs::read_to_string(svg_path) else {
        return false;
    };
    let (r, g, b) = extract_svg_accent_color(&svg_text);
    let png_bytes = encode_solid_rounded_png(out_size.clamp(16, 128), r, g, b);
    std::fs::write(out_file, png_bytes).is_ok()
}

pub fn extract_svg_accent_color(svg_text: &str) -> (u8, u8, u8) {
    for chunk in svg_text.split('#').skip(1) {
        let hex: String = chunk.chars().take_while(|c| c.is_ascii_hexdigit()).collect();
        if hex.len() == 6 {
            if let (Ok(r), Ok(g), Ok(b)) = (
                u8::from_str_radix(&hex[0..2], 16),
                u8::from_str_radix(&hex[2..4], 16),
                u8::from_str_radix(&hex[4..6], 16),
            ) {
                if (r as u16 + g as u16 + b as u16) > 45 {
                    return (r, g, b);
                }
            }
        }
    }
    (217, 107, 67)
}

fn encode_solid_rounded_png(size: u32, r: u8, g: u8, b: u8) -> Vec<u8> {
    let mut raw_scanlines = Vec::with_capacity(((size * 4 + 1) * size) as usize);
    let center = (size as f32 - 1.0) * 0.5;
    let radius = center * 0.85;
    for y in 0..size {
        raw_scanlines.push(0u8); // filter type None
        for x in 0..size {
            let dx = (x as f32 - center).abs();
            let dy = (y as f32 - center).abs();
            let dist = (dx * dx + dy * dy).sqrt();
            if dist <= radius {
                raw_scanlines.extend_from_slice(&[r, g, b, 240]);
            } else {
                raw_scanlines.extend_from_slice(&[0, 0, 0, 0]);
            }
        }
    }

    let mut png = Vec::new();
    png.extend_from_slice(&[137, 80, 78, 71, 13, 10, 26, 10]);
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&size.to_be_bytes());
    ihdr.extend_from_slice(&size.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    write_png_chunk(&mut png, b"IHDR", &ihdr);

    // Stored zlib stream (uncompressed deflate blocks)
    let mut zlib = vec![0x78, 0x01];
    let chunks: Vec<&[u8]> = raw_scanlines.chunks(65535).collect();
    for (i, chunk) in chunks.iter().enumerate() {
        let is_last = i + 1 == chunks.len();
        zlib.push(if is_last { 0x01 } else { 0x00 });
        let len = chunk.len() as u16;
        zlib.extend_from_slice(&len.to_le_bytes());
        zlib.extend_from_slice(&(!len).to_le_bytes());
        zlib.extend_from_slice(chunk);
    }
    let adler = adler32(&raw_scanlines);
    zlib.extend_from_slice(&adler.to_be_bytes());
    write_png_chunk(&mut png, b"IDAT", &zlib);
    write_png_chunk(&mut png, b"IEND", &[]);
    png
}

fn write_png_chunk(out: &mut Vec<u8>, tag: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(tag);
    out.extend_from_slice(data);
    let mut crc = 0xFFFF_FFFFu32;
    for &b in tag.iter().chain(data.iter()) {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if (crc & 1) != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    out.extend_from_slice(&(!crc).to_be_bytes());
}

fn adler32(data: &[u8]) -> u32 {
    let mut a: u32 = 1;
    let mut b: u32 = 0;
    for &byte in data {
        a = (a + byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

static ICON_CACHE: std::sync::Mutex<Option<std::collections::HashMap<String, Option<PathBuf>>>> =
    std::sync::Mutex::new(None);

pub fn resolve_desktop_icon(icon_name: Option<&str>, name: &str, exec: &str) -> Option<PathBuf> {
    let key = format!("{}:{}:{}", icon_name.unwrap_or(""), name, exec);
    {
        if let Ok(mut guard) = ICON_CACHE.lock() {
            let map = guard.get_or_insert_with(std::collections::HashMap::new);
            if let Some(cached) = map.get(&key) {
                return cached.clone();
            }
        }
    }

    let result = resolve_desktop_icon_uncached(icon_name, name, exec);

    if let Ok(mut guard) = ICON_CACHE.lock() {
        let map = guard.get_or_insert_with(std::collections::HashMap::new);
        map.insert(key, result.clone());
    }
    result
}

fn resolve_desktop_icon_uncached(icon_name: Option<&str>, name: &str, exec: &str) -> Option<PathBuf> {
    // 1. Direct path check
    if let Some(ic) = icon_name {
        if ic.starts_with('/') {
            let p = PathBuf::from(ic);
            if p.exists() {
                if ic.ends_with(".svg") {
                    return convert_svg_to_png(&p, 64);
                }
                return Some(p);
            }
        }
    }

    let raw_icon = icon_name.unwrap_or("").trim();
    let clean_icon = raw_icon.strip_suffix(".png")
        .or_else(|| raw_icon.strip_suffix(".svg"))
        .unwrap_or(raw_icon);

    // 1.6 Flatpak exports & Corten Forge cached PNG icons
    if !clean_icon.is_empty() {
        let home = std::env::var("HOME").unwrap_or_default();
        let flatpak_candidates = [
            format!("{}/.local/share/flatpak/exports/share/icons/hicolor/128x128/apps/{}.png", home, clean_icon),
            format!("{}/.local/share/flatpak/exports/share/icons/hicolor/64x64/apps/{}.png", home, clean_icon),
            format!("/var/lib/flatpak/exports/share/icons/hicolor/128x128/apps/{}.png", clean_icon),
            format!("/var/lib/flatpak/exports/share/icons/hicolor/64x64/apps/{}.png", clean_icon),
            format!("{}/.cache/corten-forge/icons/{}.img", home, clean_icon),
        ];
        for fp in &flatpak_candidates {
            let p = PathBuf::from(fp);
            if p.exists() {
                return Some(p);
            }
        }
    }

    let lower_name = name.to_lowercase();
    let lower_exec = exec.to_lowercase();
    let lower_icon = clean_icon.to_lowercase();
    let combined = format!("{} {} {}", lower_icon, lower_name, lower_exec);

    // 1.5 Direct Corten Bespoke Icon check
    let bespoke_candidates = [
        format!("assets/icons/hicolor/64x64/apps/{}.png", clean_icon),
        format!("assets/icons/hicolor/48x48/apps/{}.png", clean_icon),
        format!("assets/icons/hicolor/scalable/apps/{}.svg", clean_icon),
        format!("assets/icons/hicolor/64x64/apps/{}.png", lower_icon),
        format!("assets/icons/hicolor/48x48/apps/{}.png", lower_icon),
        format!("assets/icons/hicolor/scalable/apps/{}.svg", lower_icon),
    ];
    for b_path_str in &bespoke_candidates {
        let p = PathBuf::from(b_path_str);
        if p.exists() {
            if b_path_str.ends_with(".svg") {
                if let Some(png) = convert_svg_to_png(&p, 64) {
                    return Some(png);
                }
            } else {
                return Some(p);
            }
        }
    }

    // 2. Specific known system app mappings
    let known_candidates: &[&str] = if combined.contains("corten-ai")
        || combined.contains("ai")
        || combined.contains("assistant")
    {
        &[
            "assets/icons/hicolor/64x64/apps/corten-ai.png",
            "assets/icons/hicolor/48x48/apps/corten-ai.png",
            "/usr/share/pixmaps/corten-ai.png",
            "assets/logos/ai_concept1_aura_core.png",
            "/usr/share/pixmaps/corten.png",
            "assets/logo.png",
        ]
    } else if combined.contains("corten-terminal")
        || combined.contains("term")
        || combined.contains("alacritty")
        || combined.contains("kitty")
        || combined.contains("foot")
        || combined.contains("konsole")
    {
        &[
            "assets/icons/hicolor/64x64/apps/corten-terminal.png",
            "assets/icons/hicolor/48x48/apps/corten-terminal.png",
            "/usr/share/icons/oxygen/base/48x48/apps/utilities-terminal.png",
            "/usr/share/icons/breeze/apps/48/utilities-terminal.svg",
            "/usr/share/icons/AdwaitaLegacy/48x48/legacy/utilities-terminal.png",
            "/usr/share/icons/hicolor/48x48/apps/alacritty.png",
            "/usr/share/icons/oxygen/base/64x64/apps/utilities-terminal.png",
        ]
    } else if combined.contains("corten-files")
        || combined.contains("file")
        || combined.contains("nautilus")
        || combined.contains("dolphin")
        || combined.contains("thunar")
    {
        &[
            "assets/icons/hicolor/64x64/apps/corten-files.png",
            "assets/icons/hicolor/48x48/apps/corten-files.png",
            "/usr/share/icons/oxygen/base/48x48/apps/system-file-manager.png",
            "/usr/share/icons/breeze/apps/48/system-file-manager.svg",
            "/usr/share/icons/AdwaitaLegacy/48x48/legacy/system-file-manager.png",
            "/usr/share/icons/oxygen/base/64x64/apps/system-file-manager.png",
        ]
    } else if combined.contains("corten-settings")
        || combined.contains("setting")
        || combined.contains("config")
        || combined.contains("control")
        || combined.contains("pref")
    {
        &[
            "assets/icons/hicolor/64x64/apps/corten-settings.png",
            "assets/icons/hicolor/48x48/apps/corten-settings.png",
            "/usr/share/icons/oxygen/base/48x48/categories/preferences-system.png",
            "/usr/share/icons/breeze/preferences/32/preferences-system.svg",
            "/usr/share/icons/AdwaitaLegacy/48x48/legacy/preferences-system.png",
            "/usr/share/icons/oxygen/base/64x64/categories/preferences-system.png",
        ]
    } else if combined.contains("corten-forge")
        || combined.contains("forge")
        || combined.contains("store")
        || combined.contains("software")
        || combined.contains("package")
    {
        &[
            "assets/icons/hicolor/64x64/apps/corten-forge.png",
            "assets/icons/hicolor/48x48/apps/corten-forge.png",
            "/usr/share/icons/oxygen/base/48x48/apps/system-software-install.png",
            "/usr/share/icons/breeze/apps/48/system-software-install.svg",
            "/usr/share/icons/AdwaitaLegacy/48x48/legacy/system-software-install.png",
            "assets/logo.png",
        ]
    } else if combined.contains("corten-welcome") || combined.contains("welcome") {
        &[
            "assets/icons/hicolor/64x64/apps/corten-welcome.png",
            "assets/icons/hicolor/48x48/apps/corten-welcome.png",
        ]
    } else if combined.contains("editor")
        || combined.contains("code")
        || combined.contains("nvim")
        || combined.contains("rust")
        || combined.contains("text")
        || combined.contains("kate")
        || combined.contains("gedit")
    {
        &[
            "assets/icons/hicolor/64x64/apps/corten-editor.png",
            "assets/icons/hicolor/48x48/apps/corten-editor.png",
            "/usr/share/icons/oxygen/base/48x48/apps/accessories-text-editor.png",
            "/usr/share/icons/breeze/apps/48/accessories-text-editor.svg",
            "/usr/share/icons/AdwaitaLegacy/48x48/legacy/accessories-text-editor.png",
            "/usr/share/pixmaps/vscode.png",
            "/usr/share/icons/oxygen/base/64x64/apps/accessories-text-editor.png",
        ]
    } else if combined.contains("browser")
        || combined.contains("firefox")
        || combined.contains("chrom")
        || combined.contains("brave")
        || combined.contains("zen")
        || combined.contains("web")
    {
        &[
            "assets/icons/hicolor/64x64/apps/corten-browser.png",
            "assets/icons/hicolor/48x48/apps/corten-browser.png",
            "/usr/share/icons/hicolor/48x48/apps/firefox.png",
            "/usr/share/icons/oxygen/base/48x48/apps/internet-web-browser.png",
            "/usr/share/icons/breeze/apps/48/internet-web-browser.svg",
            "/usr/share/icons/oxygen/base/64x64/apps/internet-web-browser.png",
        ]
    } else if combined.contains("system-monitor") || combined.contains("htop") || combined.contains("monitor") {
        &[
            "assets/icons/hicolor/64x64/apps/corten-monitor.png",
            "assets/icons/hicolor/48x48/apps/corten-monitor.png",
            "/usr/share/icons/oxygen/base/48x48/apps/utilities-system-monitor.png",
            "/usr/share/icons/breeze/apps/48/utilities-system-monitor.svg",
            "/usr/share/pixmaps/htop.png",
        ]
    } else {
        &[]
    };

    for path_str in known_candidates {
        let p = PathBuf::from(path_str);
        if p.exists() {
            if path_str.ends_with(".svg") {
                if let Some(png) = convert_svg_to_png(&p, 64) {
                    return Some(png);
                }
            } else {
                return Some(p);
            }
        }
    }

    // 3. Freedesktop Theme Search for `clean_icon` (PNG and SVG)
    if !clean_icon.is_empty() {
        let theme_bases = [
            "/usr/share/icons/breeze/apps/48",
            "/usr/share/icons/breeze/preferences/32",
            "/usr/share/icons/hicolor/scalable/apps",
            "/usr/share/icons/hicolor/48x48/apps",
            "/usr/share/icons/hicolor/64x64/apps",
            "/usr/share/icons/hicolor/128x128/apps",
            "/usr/share/icons/hicolor/32x32/apps",
            "/usr/share/icons/oxygen/base/48x48/apps",
            "/usr/share/icons/oxygen/base/48x48/categories",
            "/usr/share/icons/oxygen/base/64x64/apps",
            "/usr/share/icons/oxygen/base/32x32/apps",
            "/usr/share/icons/Adwaita/scalable/apps",
            "/usr/share/icons/AdwaitaLegacy/48x48/legacy",
            "/usr/share/icons/AdwaitaLegacy/32x32/legacy",
            "/usr/share/pixmaps",
        ];

        for base in theme_bases {
            // PNG check
            let p1 = PathBuf::from(format!("{}/{}.png", base, clean_icon));
            if p1.exists() {
                return Some(p1);
            }
            let p2 = PathBuf::from(format!("{}/{}.png", base, lower_icon));
            if p2.exists() {
                return Some(p2);
            }

            // SVG check with auto rasterization
            let s1 = PathBuf::from(format!("{}/{}.svg", base, clean_icon));
            if s1.exists() {
                if let Some(png) = convert_svg_to_png(&s1, 64) {
                    return Some(png);
                }
            }
            let s2 = PathBuf::from(format!("{}/{}.svg", base, lower_icon));
            if s2.exists() {
                if let Some(png) = convert_svg_to_png(&s2, 64) {
                    return Some(png);
                }
            }

            // Reverse DNS check e.g. org.gnome.Terminal -> terminal
            if let Some(last_part) = clean_icon.split('.').next_back() {
                let p3 = PathBuf::from(format!("{}/{}.png", base, last_part.to_lowercase()));
                if p3.exists() {
                    return Some(p3);
                }
                let s3 = PathBuf::from(format!("{}/{}.svg", base, last_part.to_lowercase()));
                if s3.exists() {
                    if let Some(png) = convert_svg_to_png(&s3, 64) {
                        return Some(png);
                    }
                }
            }
        }
    }

    // 4. Pixmaps check by lower_exec
    let first_exec_word = lower_exec.split_whitespace().next().unwrap_or("");
    let exec_clean = Path::new(first_exec_word)
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or(first_exec_word);

    let p_exec = PathBuf::from(format!("/usr/share/pixmaps/{}.png", exec_clean));
    if p_exec.exists() {
        return Some(p_exec);
    }

    None
}

/// Scans standard XDG data directories for .desktop files and parses them
pub fn scan_desktop_entries() -> Vec<AppEntry> {
    let mut search_paths = Vec::new();

    // 1. User local applications & user Flatpak exports
    if let Some(base_dirs) = directories::BaseDirs::new() {
        search_paths.push(base_dirs.data_dir().join("applications"));
        search_paths.push(
            base_dirs
                .data_dir()
                .join("flatpak/exports/share/applications"),
        );
    }

    // 2. System & system Flatpak application directories
    search_paths.push(PathBuf::from("/var/lib/flatpak/exports/share/applications"));
    search_paths.push(PathBuf::from("/usr/local/share/applications"));
    search_paths.push(PathBuf::from("/usr/share/applications"));

    let mut entries = Vec::new();
    let mut seen_ids = HashSet::new();

    // Built-in Corten Desktop native applications (always available even without installed .desktop files)
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()));
    let resolve_bin = |bin_name: &str| -> String {
        if let Some(d) = &exe_dir {
            let p = d.join(bin_name);
            if p.is_file() {
                return p.to_string_lossy().to_string();
            }
        }
        if let Ok(cur) = std::env::current_dir() {
            let p = cur.join(bin_name);
            if p.is_file() {
                return p.to_string_lossy().to_string();
            }
            let p_rel = cur.join("target/release").join(bin_name);
            if p_rel.is_file() {
                return p_rel.to_string_lossy().to_string();
            }
            let p_deb = cur.join("target/debug").join(bin_name);
            if p_deb.is_file() {
                return p_deb.to_string_lossy().to_string();
            }
        }
        if let Some(paths) = std::env::var_os("PATH") {
            for path in std::env::split_paths(&paths) {
                let candidate = path.join(bin_name);
                if candidate.is_file() {
                    return candidate.to_string_lossy().to_string();
                }
            }
        }
        for sys_dir in &["/usr/local/bin", "/usr/bin", "/bin"] {
            let candidate = PathBuf::from(sys_dir).join(bin_name);
            if candidate.is_file() {
                return candidate.to_string_lossy().to_string();
            }
        }
        bin_name.to_string()
    };

    entries.push(AppEntry {
        id: "corten-terminal.desktop".to_string(),
        name: "Corten Terminal".to_string(),
        generic_name: Some("Terminal".to_string()),
        comment: Some("Corten Wayland yerel GPU-hızlandırmalı rust terminali".to_string()),
        exec: resolve_bin("corten-terminal"),
        icon: Some("corten-terminal".to_string()),
        icon_path: resolve_desktop_icon(Some("corten-terminal"), "Corten Terminal", "corten-terminal"),
        terminal: false,
        categories: vec!["System".to_string(), "TerminalEmulator".to_string()],
    });
    seen_ids.insert("corten-terminal.desktop".to_string());

    entries.push(AppEntry {
        id: "corten-files.desktop".to_string(),
        name: "Corten Files".to_string(),
        generic_name: Some("Dosya Yöneticisi".to_string()),
        comment: Some("Corten Wayland yerel dosya gezgini ve yöneticisi".to_string()),
        exec: resolve_bin("corten-files"),
        icon: Some("corten-files".to_string()),
        icon_path: resolve_desktop_icon(Some("corten-files"), "Corten Files", "corten-files"),
        terminal: false,
        categories: vec!["System".to_string(), "Utility".to_string(), "FileManager".to_string()],
    });
    seen_ids.insert("corten-files.desktop".to_string());

    entries.push(AppEntry {
        id: "corten-settings.desktop".to_string(),
        name: "Corten Settings".to_string(),
        generic_name: Some("Sistem Ayarları".to_string()),
        comment: Some("Corten Wayland masaüstü ayarları ve yapılandırması".to_string()),
        exec: resolve_bin("corten-settings"),
        icon: Some("corten-settings".to_string()),
        icon_path: resolve_desktop_icon(Some("corten-settings"), "Corten Settings", "corten-settings"),
        terminal: false,
        categories: vec!["System".to_string(), "Settings".to_string()],
    });
    seen_ids.insert("corten-settings.desktop".to_string());

    entries.push(AppEntry {
        id: "corten-monitor.desktop".to_string(),
        name: "Corten Monitor".to_string(),
        generic_name: Some("Görev Yöneticisi".to_string()),
        comment: Some("Corten Wayland sistem kaynak izleyici ve görev yöneticisi".to_string()),
        exec: resolve_bin("corten-monitor"),
        icon: Some("corten-monitor".to_string()),
        icon_path: resolve_desktop_icon(Some("corten-monitor"), "Corten Monitor", "corten-monitor"),
        terminal: false,
        categories: vec!["System".to_string(), "Monitor".to_string()],
    });
    seen_ids.insert("corten-monitor.desktop".to_string());

    entries.push(AppEntry {
        id: "corten-welcome.desktop".to_string(),
        name: "Corten Welcome".to_string(),
        generic_name: Some("Hoş Geldiniz Sihirbazı".to_string()),
        comment: Some("Corten Wayland ilk açılış rehberi ve tanıtım sihirbazı".to_string()),
        exec: resolve_bin("corten-welcome"),
        icon: Some("corten-welcome".to_string()),
        icon_path: resolve_desktop_icon(Some("corten-welcome"), "Corten Welcome", "corten-welcome"),
        terminal: false,
        categories: vec!["System".to_string(), "Settings".to_string(), "Documentation".to_string()],
    });
    seen_ids.insert("corten-welcome.desktop".to_string());

    entries.push(AppEntry {
        id: "corten-ai.desktop".to_string(),
        name: "Corten AI".to_string(),
        generic_name: Some("Yapay Zeka Asistanı".to_string()),
        comment: Some("Corten Wayland yerel AI masaüstü asistanı".to_string()),
        exec: resolve_bin("corten-ai"),
        icon: Some("corten-ai".to_string()),
        icon_path: resolve_desktop_icon(Some("corten-ai"), "Corten AI", "corten-ai"),
        terminal: false,
        categories: vec!["System".to_string(), "Utility".to_string(), "AI".to_string()],
    });
    seen_ids.insert("corten-ai.desktop".to_string());

    entries.push(AppEntry {
        id: "corten-forge.desktop".to_string(),
        name: "Corten Forge".to_string(),
        generic_name: Some("Yazılım & Paket Merkezi".to_string()),
        comment: Some("Wayland Yazılım & Paket Merkezi (Flatpak, Arch & AI)".to_string()),
        exec: resolve_bin("corten-forge"),
        icon: Some("corten-forge".to_string()),
        icon_path: resolve_desktop_icon(Some("corten-forge"), "Corten Forge", "corten-forge"),
        terminal: false,
        categories: vec!["System".to_string(), "PackageManager".to_string(), "Settings".to_string()],
    });
    seen_ids.insert("corten-forge.desktop".to_string());

    for dir in search_paths {
        if dir.exists() && dir.is_dir() {
            scan_dir(&dir, &mut entries, &mut seen_ids);
        }
    }

    debug!("Found {} unique desktop applications", entries.len());
    entries
}

fn scan_dir(dir: &Path, entries: &mut Vec<AppEntry>, seen_ids: &mut HashSet<String>) {
    let read_dir = match fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) => {
            warn!("Failed to read directory {:?}: {}", dir, e);
            return;
        }
    };

    for entry in read_dir.flatten() {
        let path = entry.path();
        if path.is_file() && path.extension().and_then(|ext| ext.to_str()) == Some("desktop") {
            let file_name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();

            if seen_ids.contains(&file_name) {
                continue;
            }

            if let Some(app) = parse_desktop_file(&path, &file_name) {
                seen_ids.insert(file_name);
                entries.push(app);
            }
        }
    }
}

fn parse_desktop_file(path: &Path, file_id: &str) -> Option<AppEntry> {
    let entry = freedesktop_desktop_entry::DesktopEntry::from_path(path, None::<&[&str]>).ok()?;

    // Ignore apps flagged with NoDisplay=true
    if entry.no_display() {
        return None;
    }

    let name = entry.name(&[""])?.to_string();
    let exec = entry.exec()?.to_string();
    let generic_name = entry.generic_name(&[""]).map(|s| s.to_string());
    let comment = entry.comment(&[""]).map(|s| s.to_string());
    let icon = entry.icon().map(|s| s.to_string());
    let terminal = entry.terminal();
    let categories = entry
        .categories()
        .map(|cats| cats.into_iter().map(|s| s.to_string()).collect())
        .unwrap_or_default();

    let icon_path = resolve_desktop_icon(icon.as_deref(), &name, &exec);

    Some(AppEntry {
        id: file_id.to_string(),
        name,
        generic_name,
        comment,
        exec,
        icon,
        icon_path,
        terminal,
        categories,
    })
}

#[cfg(test)]
mod tests {
    use super::parse_exec;

    #[test]
    fn parses_quotes_and_field_codes_without_shell() {
        assert_eq!(
            parse_exec("demo --title 'Hello world' %U").unwrap(),
            vec!["demo", "--title", "Hello world"]
        );
        assert_eq!(
            parse_exec("demo \"a\\\"b\" %%").unwrap(),
            vec!["demo", "a\"b", "%"]
        );
    }

    #[test]
    fn rejects_unterminated_quotes() {
        assert!(parse_exec("demo 'broken").is_err());
    }

    #[test]
    fn test_icon_caching() {
        let first = super::resolve_desktop_icon(Some("utilities-terminal"), "Corten Terminal", "corten-terminal");
        let second = super::resolve_desktop_icon(Some("utilities-terminal"), "Corten Terminal", "corten-terminal");
        assert_eq!(first, second);
    }
}
