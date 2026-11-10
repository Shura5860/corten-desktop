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
    /// Launches the application by parsing the Exec line with robust fallback
    pub fn launch(&self) -> std::io::Result<()> {
        let args = parse_exec(&self.exec)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
        let (program, arguments) = args.split_first().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "empty Exec entry")
        })?;

        info!("Launching app '{}': {}", self.name, program);

        let wayland_display = std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "corten-0".into());

        let mut cmd = Command::new(program);
        cmd.args(arguments)
            .env("WAYLAND_DISPLAY", &wayland_display)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());

        match cmd.spawn() {
            Ok(_) => Ok(()),
            Err(e) => {
                tracing::warn!("Direct launch failed for {}: {}. Attempting shell fallback...", program, e);
                let sanitized = self.exec
                    .replace("%u", "")
                    .replace("%U", "")
                    .replace("%f", "")
                    .replace("%F", "")
                    .replace("%i", "")
                    .replace("%c", "")
                    .replace("%k", "");
                let mut shell_cmd = Command::new("sh");
                shell_cmd.arg("-c")
                    .arg(format!("{} &", sanitized.trim()))
                    .env("WAYLAND_DISPLAY", &wayland_display)
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null());
                shell_cmd.spawn().map(|_| ())
            }
        }
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
                "f" | "F" | "u" | "U" | "i" | "c" | "k" => continue,
                _ => return Err(format!("unsupported desktop-entry field code: %{code}")),
            }
        } else {
            expanded.push(arg);
        }
    }
    Ok(expanded)
}

fn convert_svg_to_png(svg_path: &std::path::Path, out_size: u32) -> Option<PathBuf> {
    let cache_dir = PathBuf::from("/tmp/corten_icon_cache");
    let _ = std::fs::create_dir_all(&cache_dir);

    let file_stem = svg_path.file_stem()?.to_string_lossy();
    let out_file = cache_dir.join(format!("{}_{}x{}.png", file_stem, out_size, out_size));
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
        .ok()?;

    if status.success() && out_file.exists() {
        Some(out_file)
    } else {
        None
    }
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

    let lower_name = name.to_lowercase();
    let lower_exec = exec.to_lowercase();
    let lower_icon = clean_icon.to_lowercase();
    let combined = format!("{} {} {}", lower_icon, lower_name, lower_exec);

    // 2. Specific known system app mappings
    let known_candidates: &[&str] = if combined.contains("corten-ai")
        || combined.contains("ai")
        || combined.contains("assistant")
    {
        &[
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
            "/usr/share/icons/oxygen/base/48x48/categories/preferences-system.png",
            "/usr/share/icons/breeze/preferences/32/preferences-system.svg",
            "/usr/share/icons/AdwaitaLegacy/48x48/legacy/preferences-system.png",
            "/usr/share/icons/oxygen/base/64x64/categories/preferences-system.png",
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
            "/usr/share/icons/hicolor/48x48/apps/firefox.png",
            "/usr/share/icons/oxygen/base/48x48/apps/internet-web-browser.png",
            "/usr/share/icons/breeze/apps/48/internet-web-browser.svg",
            "/usr/share/icons/oxygen/base/64x64/apps/internet-web-browser.png",
        ]
    } else if combined.contains("system-monitor") || combined.contains("htop") || combined.contains("monitor") {
        &[
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

    // 1. User local applications
    if let Some(base_dirs) = directories::BaseDirs::new() {
        search_paths.push(base_dirs.data_dir().join("applications"));
    }

    // 2. System application directories
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
        icon: Some("utilities-terminal".to_string()),
        icon_path: resolve_desktop_icon(Some("utilities-terminal"), "Corten Terminal", "corten-terminal"),
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
        icon: Some("system-file-manager".to_string()),
        icon_path: resolve_desktop_icon(Some("system-file-manager"), "Corten Files", "corten-files"),
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
        icon: Some("preferences-system".to_string()),
        icon_path: resolve_desktop_icon(Some("preferences-system"), "Corten Settings", "corten-settings"),
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
        icon: Some("utilities-system-monitor".to_string()),
        icon_path: resolve_desktop_icon(Some("utilities-system-monitor"), "Corten Monitor", "corten-monitor"),
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
        icon: Some("preferences-desktop".to_string()),
        icon_path: resolve_desktop_icon(Some("preferences-desktop"), "Corten Welcome", "corten-welcome"),
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
