use std::fs;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ViewMode {
    #[default]
    List,
    Grid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum SortColumn {
    #[default]
    Name,
    Size,
    Modified,
    Kind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum SortDirection {
    #[default]
    Ascending,
    Descending,
}

impl SortDirection {
    pub fn toggle(self) -> Self {
        match self {
            Self::Ascending => Self::Descending,
            Self::Descending => Self::Ascending,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EntryKind {
    Directory,
    Code,
    Document,
    Image,
    Archive,
    AudioVideo,
    Executable,
    File,
}

impl EntryKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Directory => "Klasör",
            Self::Code => "Kaynak Kod",
            Self::Document => "Belge",
            Self::Image => "Görsel",
            Self::Archive => "Arşiv",
            Self::AudioVideo => "Medya",
            Self::Executable => "Çalıştırılabilir",
            Self::File => "Dosya",
        }
    }

    pub fn badge(self) -> &'static str {
        match self {
            Self::Directory => "DIR",
            Self::Code => "CODE",
            Self::Document => "DOC",
            Self::Image => "IMG",
            Self::Archive => "ZIP",
            Self::AudioVideo => "AV",
            Self::Executable => "BIN",
            Self::File => "FILE",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    pub name: String,
    pub path: PathBuf,
    pub kind: EntryKind,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub is_hidden: bool,
    pub size: u64,
    pub modified_secs: u64,
    pub permissions: String,
}

impl FileEntry {
    pub fn from_path(path: PathBuf) -> Option<Self> {
        let symlink_meta = fs::symlink_metadata(&path).ok()?;
        let is_symlink = symlink_meta.file_type().is_symlink();
        let meta = fs::metadata(&path).unwrap_or(symlink_meta);
        let is_dir = meta.is_dir();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string_lossy().to_string());
        let is_hidden = name.starts_with('.');
        let size = if is_dir { 0 } else { meta.len() };
        let modified_secs = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let mode = meta.permissions().mode();
        let permissions = format_unix_mode(mode, is_dir, is_symlink);
        let kind = classify_entry(&path, is_dir, mode);

        Some(Self {
            name,
            path,
            kind,
            is_dir,
            is_symlink,
            is_hidden,
            size,
            modified_secs,
            permissions,
        })
    }

    pub fn is_archive(&self) -> bool {
        self.kind == EntryKind::Archive
    }

    pub fn is_image(&self) -> bool {
        self.kind == EntryKind::Image
    }
}

/// Returns the preview path for an image file if supported and reasonably sized (< 16 MB)
pub fn get_image_preview_path(path: &Path) -> Option<PathBuf> {
    let ext = path.extension()?.to_str()?;
    if matches!(
        ext,
        "png" | "PNG" | "jpg" | "JPG" | "jpeg" | "JPEG" | "webp" | "WEBP" | "bmp" | "BMP" | "ico" | "ICO"
    ) {
        if let Ok(meta) = fs::metadata(path) {
            // Guard against massive images eating memory in the grid view
            if meta.len() <= 16 * 1024 * 1024 {
                return Some(path.to_path_buf());
            }
        }
    }
    None
}

#[derive(Debug, Clone)]
pub struct EntryInspection {
    pub name: String,
    pub full_path: String,
    pub kind_label: &'static str,
    pub size_display: String,
    pub permissions: String,
    pub modified_display: String,
    pub text_preview: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidebarPlace {
    pub label: String,
    pub path: PathBuf,
    pub is_trash: bool,
    pub is_mount: bool,
}

pub fn classify_entry(path: &Path, is_dir: bool, mode: u32) -> EntryKind {
    if is_dir {
        return EntryKind::Directory;
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();

    match ext.as_str() {
        "rs" | "toml" | "py" | "js" | "ts" | "tsx" | "jsx" | "c" | "cpp" | "h" | "hpp" | "go"
        | "sh" | "zsh" | "fish" | "json" | "yaml" | "yml" | "html" | "css" | "scss" | "sql"
        | "lua" | "zig" | "nix" => EntryKind::Code,
        "md" | "txt" | "pdf" | "doc" | "docx" | "odt" | "rtf" | "csv" | "log" | "conf" | "ini" => {
            EntryKind::Document
        }
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp" | "ico" => EntryKind::Image,
        "zip" | "tar" | "gz" | "tgz" | "xz" | "bz2" | "7z" | "rar" | "zst" => EntryKind::Archive,
        "mp3" | "wav" | "flac" | "ogg" | "mp4" | "mkv" | "webm" | "avi" | "mov" => {
            EntryKind::AudioVideo
        }
        _ if (mode & 0o111) != 0 => EntryKind::Executable,
        _ => EntryKind::File,
    }
}

pub fn format_unix_mode(mode: u32, is_dir: bool, is_symlink: bool) -> String {
    let file_type = if is_symlink {
        'l'
    } else if is_dir {
        'd'
    } else {
        '-'
    };
    let flags = [
        (0o400, 'r'),
        (0o200, 'w'),
        (0o100, 'x'),
        (0o040, 'r'),
        (0o020, 'w'),
        (0o010, 'x'),
        (0o004, 'r'),
        (0o002, 'w'),
        (0o001, 'x'),
    ];
    let mut out = String::with_capacity(10);
    out.push(file_type);
    for (mask, ch) in flags {
        out.push(if (mode & mask) != 0 { ch } else { '-' });
    }
    out
}

pub fn format_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;

    if bytes < 1024 {
        format!("{bytes} B")
    } else if (bytes as f64) < MB {
        format!("{:.1} KB", bytes as f64 / KB)
    } else if (bytes as f64) < GB {
        format!("{:.1} MB", bytes as f64 / MB)
    } else {
        format!("{:.2} GB", bytes as f64 / GB)
    }
}

pub fn format_relative_time(modified_secs: u64) -> String {
    if modified_secs == 0 {
        return "Bilinmiyor".to_string();
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(modified_secs);
    let diff = now.saturating_sub(modified_secs);
    if diff < 60 {
        "Az önce".to_string()
    } else if diff < 3600 {
        format!("{} dk önce", diff / 60)
    } else if diff < 86_400 {
        format!("{} sa önce", diff / 3600)
    } else if diff < 86_400 * 30 {
        format!("{} gün önce", diff / 86_400)
    } else {
        format!("{} ay önce", (diff / (86_400 * 30)).max(1))
    }
}

pub fn read_directory(
    dir: &Path,
    show_hidden: bool,
    search_query: &str,
    sort_col: SortColumn,
    sort_dir: SortDirection,
) -> Result<Vec<FileEntry>, String> {
    let read_dir = fs::read_dir(dir).map_err(|e| format!("Dizin okunamadı: {e}"))?;
    let query = search_query.trim().to_lowercase();
    let mut entries = Vec::new();

    for item in read_dir.flatten() {
        let Some(entry) = FileEntry::from_path(item.path()) else {
            continue;
        };
        if !show_hidden && entry.is_hidden {
            continue;
        }
        if !query.is_empty() && !entry.name.to_lowercase().contains(&query) {
            continue;
        }
        entries.push(entry);
    }

    sort_entries(&mut entries, sort_col, sort_dir);
    Ok(entries)
}

pub fn sort_entries(entries: &mut [FileEntry], sort_col: SortColumn, sort_dir: SortDirection) {
    entries.sort_by(|a, b| {
        // Directories always appear before regular files
        match (a.is_dir, b.is_dir) {
            (true, false) => return std::cmp::Ordering::Less,
            (false, true) => return std::cmp::Ordering::Greater,
            _ => {}
        }

        let ordering = match sort_col {
            SortColumn::Name => a
                .name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then_with(|| a.name.cmp(&b.name)),
            SortColumn::Size => a
                .size
                .cmp(&b.size)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())),
            SortColumn::Modified => a
                .modified_secs
                .cmp(&b.modified_secs)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())),
            SortColumn::Kind => a
                .kind
                .cmp(&b.kind)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())),
        };

        match sort_dir {
            SortDirection::Ascending => ordering,
            SortDirection::Descending => ordering.reverse(),
        }
    });
}

pub fn inspect_entry(entry: &FileEntry) -> EntryInspection {
    let size_display = if entry.is_dir {
        let count = fs::read_dir(&entry.path)
            .map(|rd| rd.flatten().count())
            .unwrap_or(0);
        format!("{count} öğe")
    } else {
        format_size(entry.size)
    };

    let text_preview = if !entry.is_dir && entry.size <= 256 * 1024 {
        read_text_preview(&entry.path, 55)
    } else {
        None
    };

    EntryInspection {
        name: entry.name.clone(),
        full_path: entry.path.to_string_lossy().to_string(),
        kind_label: entry.kind.label(),
        size_display,
        permissions: entry.permissions.clone(),
        modified_display: format_relative_time(entry.modified_secs),
        text_preview,
    }
}

fn read_text_preview(path: &Path, max_lines: usize) -> Option<String> {
    let mut file = fs::File::open(path).ok()?;
    let mut buf = Vec::with_capacity(8192);
    file.by_ref().take(8192).read_to_end(&mut buf).ok()?;
    if buf.is_empty() {
        return Some("(Boş dosya)".to_string());
    }
    // Reject binary files containing NUL bytes
    if buf.contains(&0) {
        return None;
    }
    let text = std::str::from_utf8(&buf).ok()?;
    let preview: Vec<&str> = text.lines().take(max_lines).collect();
    Some(preview.join("\n"))
}

pub fn trash_files_dir() -> PathBuf {
    if let Some(base) = directories::BaseDirs::new() {
        base.data_local_dir().join("Trash").join("files")
    } else {
        PathBuf::from("/tmp/corten-trash/files")
    }
}

pub fn trash_info_dir() -> PathBuf {
    if let Some(base) = directories::BaseDirs::new() {
        base.data_local_dir().join("Trash").join("info")
    } else {
        PathBuf::from("/tmp/corten-trash/info")
    }
}

pub fn standard_places() -> Vec<SidebarPlace> {
    let mut places = Vec::new();
    let home = directories::BaseDirs::new()
        .map(|b| b.home_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("/"));

    places.push(SidebarPlace {
        label: "Ev (Home)".into(),
        path: home.clone(),
        is_trash: false,
        is_mount: false,
    });

    for (label, rel) in [
        ("Masaüstü", "Desktop"),
        ("Belgeler", "Documents"),
        ("İndirilenler", "Downloads"),
        ("Resimler", "Pictures"),
        ("Projeler", "Projects"),
    ] {
        let candidate = home.join(rel);
        if candidate.exists() {
            places.push(SidebarPlace {
                label: label.into(),
                path: candidate,
                is_trash: false,
                is_mount: false,
            });
        }
    }

    let trash = trash_files_dir();
    let _ = fs::create_dir_all(&trash);
    places.push(SidebarPlace {
        label: "Çöp Kutusu".into(),
        path: trash,
        is_trash: true,
        is_mount: false,
    });

    for mount in discover_mounts() {
        places.push(mount);
    }

    places
}

pub fn discover_mounts() -> Vec<SidebarPlace> {
    let mut mounts = Vec::new();
    let user = std::env::var("USER").unwrap_or_default();
    let mut roots = vec![PathBuf::from("/mnt")];
    if !user.is_empty() {
        roots.push(PathBuf::from(format!("/run/media/{user}")));
        roots.push(PathBuf::from(format!("/media/{user}")));
    }

    for root in roots {
        if let Ok(rd) = fs::read_dir(&root) {
            for item in rd.flatten() {
                let p = item.path();
                if p.is_dir() {
                    let name = p
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| "Disk".into());
                    mounts.push(SidebarPlace {
                        label: name,
                        path: p,
                        is_trash: false,
                        is_mount: true,
                    });
                }
            }
        }
    }
    mounts
}

pub fn unique_destination(dest_dir: &Path, file_name: &str) -> PathBuf {
    let candidate = dest_dir.join(file_name);
    if !candidate.exists() {
        return candidate;
    }

    let path_obj = Path::new(file_name);
    let stem = path_obj
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| file_name.to_string());
    let ext = path_obj
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();

    for idx in 1..1000 {
        let suffix = if idx == 1 {
            " (kopya)".to_string()
        } else {
            format!(" (kopya {idx})")
        };
        let next = dest_dir.join(format!("{stem}{suffix}{ext}"));
        if !next.exists() {
            return next;
        }
    }
    dest_dir.join(format!("{file_name}.copy"))
}

pub fn copy_entry_recursive(src: &Path, dest_dir: &Path) -> Result<PathBuf, String> {
    let file_name = src
        .file_name()
        .ok_or_else(|| "Geçersiz kaynak dosya adı".to_string())?
        .to_string_lossy()
        .to_string();
    let target = unique_destination(dest_dir, &file_name);
    copy_path_to(src, &target)?;
    Ok(target)
}

fn copy_path_to(src: &Path, dst: &Path) -> Result<(), String> {
    let meta = fs::symlink_metadata(src).map_err(|e| format!("Meta veri okunamadı: {e}"))?;
    if meta.is_dir() {
        fs::create_dir_all(dst).map_err(|e| format!("Klasör oluşturulamadı: {e}"))?;
        let rd = fs::read_dir(src).map_err(|e| format!("Klasör okunamadı: {e}"))?;
        for item in rd.flatten() {
            let child_src = item.path();
            let child_dst = dst.join(item.file_name());
            copy_path_to(&child_src, &child_dst)?;
        }
        Ok(())
    } else {
        fs::copy(src, dst)
            .map(|_| ())
            .map_err(|e| format!("Dosya kopyalanamadı: {e}"))
    }
}

pub fn move_entry(src: &Path, dest_dir: &Path) -> Result<PathBuf, String> {
    let file_name = src
        .file_name()
        .ok_or_else(|| "Geçersiz kaynak dosya adı".to_string())?
        .to_string_lossy()
        .to_string();
    let target = unique_destination(dest_dir, &file_name);
    match fs::rename(src, &target) {
        Ok(()) => Ok(target),
        Err(_) => {
            // Cross-device fallback: recursive copy + delete source
            copy_path_to(src, &target)?;
            delete_permanently(src)?;
            Ok(target)
        }
    }
}

pub fn create_folder(parent: &Path, name: &str) -> Result<PathBuf, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.contains('/') {
        return Err("Geçersiz klasör adı".into());
    }
    let target = parent.join(trimmed);
    if target.exists() {
        return Err("Bu isimde bir öğe zaten var".into());
    }
    fs::create_dir(&target).map_err(|e| format!("Klasör oluşturulamadı: {e}"))?;
    Ok(target)
}

pub fn create_file(parent: &Path, name: &str) -> Result<PathBuf, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.contains('/') {
        return Err("Geçersiz dosya adı".into());
    }
    let target = parent.join(trimmed);
    if target.exists() {
        return Err("Bu isimde bir öğe zaten var".into());
    }
    fs::File::create(&target).map_err(|e| format!("Dosya oluşturulamadı: {e}"))?;
    Ok(target)
}

pub fn rename_entry(src: &Path, new_name: &str) -> Result<PathBuf, String> {
    let trimmed = new_name.trim();
    if trimmed.is_empty() || trimmed.contains('/') {
        return Err("Geçersiz yeni ad".into());
    }
    let parent = src
        .parent()
        .ok_or_else(|| "Üst dizin bulunamadı".to_string())?;
    let target = parent.join(trimmed);
    if target == src {
        return Ok(target);
    }
    if target.exists() {
        return Err("Hedef isim zaten kullanılıyor".into());
    }
    fs::rename(src, &target).map_err(|e| format!("Yeniden adlandırılamadı: {e}"))?;
    Ok(target)
}

pub fn move_to_trash(src: &Path) -> Result<PathBuf, String> {
    move_to_trash_in(src, &trash_files_dir(), &trash_info_dir())
}

pub fn move_to_trash_in(
    src: &Path,
    files_dir: &Path,
    info_dir: &Path,
) -> Result<PathBuf, String> {
    fs::create_dir_all(files_dir).map_err(|e| format!("Çöp dizini oluşturulamadı: {e}"))?;
    fs::create_dir_all(info_dir).map_err(|e| format!("Çöp bilgi dizini oluşturulamadı: {e}"))?;

    let file_name = src
        .file_name()
        .ok_or_else(|| "Geçersiz dosya adı".to_string())?
        .to_string_lossy()
        .to_string();
    let dest = unique_destination(files_dir, &file_name);
    let trashed_name = dest
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or(file_name);

    let info_path = info_dir.join(format!("{trashed_name}.trashinfo"));
    let abs_src = src.canonicalize().unwrap_or_else(|_| src.to_path_buf());
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    // Format YYYY-MM-DDTHH:MM:SS
    let days = now / 86400;
    let rem_secs = now % 86400;
    let hours = rem_secs / 3600;
    let minutes = (rem_secs % 3600) / 60;
    let seconds = rem_secs % 60;
    // Approximating standard date from unix epoch days
    let mut year = 1970;
    let mut d = days;
    loop {
        let leap = if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) { 366 } else { 365 };
        if d < leap {
            break;
        }
        d -= leap;
        year += 1;
    }
    let is_leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let month_days = [31, if is_leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut month = 1;
    for &md in &month_days {
        if d < md {
            break;
        }
        d -= md;
        month += 1;
    }
    let day = d + 1;
    let deletion_date = format!("{year:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{seconds:02}");
    let info_contents = format!(
        "[Trash Info]\nPath={}\nDeletionDate={deletion_date}\n",
        abs_src.to_string_lossy()
    );
    let _ = fs::write(&info_path, info_contents);

    match fs::rename(src, &dest) {
        Ok(()) => Ok(dest),
        Err(_) => {
            copy_path_to(src, &dest)?;
            delete_permanently(src)?;
            Ok(dest)
        }
    }
}

pub fn restore_from_trash(trashed_path: &Path) -> Result<PathBuf, String> {
    restore_from_trash_in(trashed_path, &trash_info_dir())
}

pub fn restore_from_trash_in(trashed_path: &Path, info_dir: &Path) -> Result<PathBuf, String> {
    let name = trashed_path
        .file_name()
        .ok_or_else(|| "Geçersiz çöp öğesi".to_string())?
        .to_string_lossy()
        .to_string();
    let info_path = info_dir.join(format!("{name}.trashinfo"));
    let info_text = fs::read_to_string(&info_path)
        .map_err(|_| "Bu öğe için geri yükleme bilgisi (.trashinfo) bulunamadı".to_string())?;

    let orig_path_str = info_text
        .lines()
        .find_map(|line| line.strip_prefix("Path="))
        .ok_or_else(|| "Geri yükleme yolu çözümlenemedi".to_string())?;

    let orig_path = PathBuf::from(orig_path_str.trim());
    let orig_parent = orig_path
        .parent()
        .ok_or_else(|| "Hedef üst klasör geçersiz".to_string())?;
    fs::create_dir_all(orig_parent).map_err(|e| format!("Hedef klasör oluşturulamadı: {e}"))?;

    let orig_name = orig_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or(name);
    let final_dest = unique_destination(orig_parent, &orig_name);

    match fs::rename(trashed_path, &final_dest) {
        Ok(()) => {
            let _ = fs::remove_file(&info_path);
            Ok(final_dest)
        }
        Err(_) => {
            copy_path_to(trashed_path, &final_dest)?;
            delete_permanently(trashed_path)?;
            let _ = fs::remove_file(&info_path);
            Ok(final_dest)
        }
    }
}

pub fn empty_trash() -> Result<(), String> {
    let files_dir = trash_files_dir();
    let info_dir = trash_info_dir();
    if files_dir.exists() {
        let _ = fs::remove_dir_all(&files_dir);
        let _ = fs::create_dir_all(&files_dir);
    }
    if info_dir.exists() {
        let _ = fs::remove_dir_all(&info_dir);
        let _ = fs::create_dir_all(&info_dir);
    }
    Ok(())
}

pub fn delete_permanently(target: &Path) -> Result<(), String> {
    let meta = fs::symlink_metadata(target).map_err(|e| format!("Dosya bulunamadı: {e}"))?;
    if meta.is_dir() {
        fs::remove_dir_all(target).map_err(|e| format!("Klasör silinemedi: {e}"))
    } else {
        fs::remove_file(target).map_err(|e| format!("Dosya silinemedi: {e}"))
    }
}

pub fn open_with_system(path: &Path) -> Result<(), String> {
    Command::new("xdg-open")
        .arg(path)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("xdg-open başlatılamadı: {e}"))
}

pub fn spawn_terminal_in(dir: &Path) -> Result<(), String> {
    let mut last_err = None;
    let wayland_display = std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "corten-0".into());
    for prog in ["corten-terminal", "alacritty", "foot", "kitty", "konsole", "xterm", "pterm", "bash"] {
        let mut cmd = Command::new(prog);
        cmd.current_dir(dir)
            .env("WAYLAND_DISPLAY", &wayland_display);
        match cmd.spawn() {
            Ok(_) => return Ok(()),
            Err(e) => last_err = Some(e),
        }
    }
    Err(format!(
        "Terminal bulunamadı: {}",
        last_err
            .map(|e| e.to_string())
            .unwrap_or_else(|| "yüklü terminal yok".into())
    ))
}

pub fn extract_archive(archive_path: &Path, dest_dir: &Path) -> Result<PathBuf, String> {
    let file_name = archive_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let lower = file_name.to_ascii_lowercase();
    let stem = lower
        .strip_suffix(".tar.gz")
        .or_else(|| lower.strip_suffix(".tar.xz"))
        .or_else(|| lower.strip_suffix(".tar.bz2"))
        .or_else(|| lower.strip_suffix(".tar.zst"))
        .or_else(|| {
            archive_path
                .file_stem()
                .and_then(|s| s.to_str())
        })
        .unwrap_or("extracted");

    let out_dir = unique_destination(dest_dir, stem);
    fs::create_dir_all(&out_dir).map_err(|e| format!("Çıkartma klasörü oluşturulamadı: {e}"))?;

    let status = if lower.ends_with(".zip") {
        Command::new("unzip")
            .arg("-q")
            .arg(archive_path)
            .arg("-d")
            .arg(&out_dir)
            .status()
    } else {
        Command::new("tar")
            .arg("-xf")
            .arg(archive_path)
            .arg("-C")
            .arg(&out_dir)
            .status()
    };

    match status {
        Ok(s) if s.success() => Ok(out_dir),
        Ok(s) => Err(format!("Arşiv çıkartma hatası (kod: {:?})", s.code())),
        Err(e) => Err(format!("Arşiv aracı çalıştırılamadı: {e}")),
    }
}

pub fn create_archive(source_path: &Path, format: &str) -> Result<PathBuf, String> {
    let parent = source_path.parent().unwrap_or_else(|| Path::new("."));
    let stem = source_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "archive".into());
    let is_zip = format == "zip";
    let ext = if is_zip { "zip" } else { "tar.gz" };
    let dest_name = format!("{}.{}", stem, ext);
    let dest_path = unique_destination(parent, &dest_name);

    let status = if is_zip {
        Command::new("zip")
            .arg("-r")
            .arg(&dest_path)
            .arg(source_path.file_name().unwrap_or_default())
            .current_dir(parent)
            .status()
    } else {
        Command::new("tar")
            .arg("-czf")
            .arg(&dest_path)
            .arg(source_path.file_name().unwrap_or_default())
            .current_dir(parent)
            .status()
    };

    match status {
        Ok(s) if s.success() => Ok(dest_path),
        Ok(s) => Err(format!("Arşivleme hatası (kod: {:?})", s.code())),
        Err(e) => Err(format!("Arşiv aracı (zip/tar) çalıştırılamadı: {e}")),
    }
}

pub fn breadcrumbs(path: &Path) -> Vec<(String, PathBuf)> {
    let mut crumbs = Vec::new();
    let mut acc = PathBuf::new();
    for comp in path.components() {
        acc.push(comp);
        let label = match comp {
            std::path::Component::RootDir => "/".to_string(),
            other => other.as_os_str().to_string_lossy().to_string(),
        };
        crumbs.push((label, acc.clone()));
    }
    if crumbs.is_empty() {
        crumbs.push(("/".to_string(), PathBuf::from("/")));
    }
    crumbs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directories_sort_before_files_and_respect_direction() {
        let tmp = std::env::temp_dir().join("corten_files_test_sort");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("zeta_dir")).unwrap();
        fs::write(tmp.join("alpha.rs"), "fn main() {}").unwrap();
        fs::write(tmp.join("beta.txt"), "hello").unwrap();

        let list = read_directory(
            &tmp,
            false,
            "",
            SortColumn::Name,
            SortDirection::Ascending,
        )
        .unwrap();
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].name, "zeta_dir");
        assert!(list[0].is_dir);
        assert_eq!(list[1].name, "alpha.rs");
        assert_eq!(list[1].kind, EntryKind::Code);
        assert_eq!(list[2].name, "beta.txt");

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn copy_rename_and_trash_roundtrip_work() {
        let tmp = std::env::temp_dir().join("corten_files_test_ops");
        let _ = fs::remove_dir_all(&tmp);
        let src_dir = tmp.join("src");
        let dst_dir = tmp.join("dst");
        let trash_files = tmp.join("trash/files");
        let trash_info = tmp.join("trash/info");
        fs::create_dir_all(&src_dir).unwrap();
        fs::create_dir_all(&dst_dir).unwrap();

        let file = create_file(&src_dir, "notes.md").unwrap();
        fs::write(&file, "# Corten Files").unwrap();

        let renamed = rename_entry(&file, "readme.md").unwrap();
        assert!(renamed.exists());

        let copied1 = copy_entry_recursive(&renamed, &dst_dir).unwrap();
        let copied2 = copy_entry_recursive(&renamed, &dst_dir).unwrap();
        assert!(copied1.exists());
        assert!(copied2.exists());
        assert_ne!(copied1, copied2);

        let trashed = move_to_trash_in(&renamed, &trash_files, &trash_info).unwrap();
        assert!(!renamed.exists());
        assert!(trashed.exists());

        let restored = restore_from_trash_in(&trashed, &trash_info).unwrap();
        assert!(restored.exists());
        assert_eq!(fs::read_to_string(&restored).unwrap(), "# Corten Files");

        let _ = fs::remove_dir_all(&tmp);
    }
}
