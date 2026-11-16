mod fs_ops;

use std::path::PathBuf;

use corten_config::ConfigManager;
use corten_theme::iced_styles::{card_style, input_style, primary_button_style, secondary_button_style};
use corten_theme::{
    Icon, Palette, PatinaVariant, Radius, Spacing, ThemeMode, Typography, LUCIDE_FONT,
    LUCIDE_FONT_BYTES, NOTO_SANS, NOTO_SANS_BYTES,
};
use fs_ops::{
    breadcrumbs, copy_entry_recursive, create_archive, create_file, create_folder,
    delete_permanently, empty_trash, extract_archive, format_relative_time, format_size,
    inspect_entry, move_entry, move_to_trash, open_with_system, query_disk_usage, read_directory,
    rename_entry, restore_from_trash, sibling_directories, spawn_terminal_in, standard_places,
    trash_files_dir, EntryInspection, EntryKind, FileEntry, SidebarPlace, SortColumn,
    SortDirection, ViewMode,
};
use iced::widget::{
    button, column, container, horizontal_space, mouse_area, row, scrollable, stack, text,
    text_input, vertical_space,
};
use iced::{border, event, keyboard, Alignment, Border, Color, Element, Event, Length, Shadow, Subscription};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FilesConfig {
    pub show_hidden: bool,
    pub show_inspector: bool,
    pub view_mode: ViewMode,
    pub sort_column: SortColumn,
    pub sort_direction: SortDirection,
    pub bookmarks: Vec<String>,
}

impl Default for FilesConfig {
    fn default() -> Self {
        Self {
            show_hidden: false,
            show_inspector: true,
            view_mode: ViewMode::List,
            sort_column: SortColumn::Name,
            sort_direction: SortDirection::Ascending,
            bookmarks: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipboardOp {
    Copy(PathBuf),
    Cut(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InlineModal {
    NewFolder(String),
    NewFile(String),
    Rename(PathBuf, String),
    ConfirmPermanentDelete(PathBuf),
    SecurityGatekeeper { path: PathBuf, sha256: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivePane {
    Left,
    Right,
}

#[derive(Debug, Clone)]
pub struct PaneState {
    pub dir: PathBuf,
    pub history_back: Vec<PathBuf>,
    pub history_forward: Vec<PathBuf>,
    pub entries: Vec<FileEntry>,
    pub selected_index: Option<usize>,
    pub search_query: String,
    pub editing_address: bool,
    pub address_input: String,
}

impl PaneState {
    pub fn new(dir: PathBuf) -> Self {
        let addr = dir.to_string_lossy().to_string();
        Self {
            dir,
            history_back: Vec::new(),
            history_forward: Vec::new(),
            entries: Vec::new(),
            selected_index: None,
            search_query: String::new(),
            editing_address: false,
            address_input: addr,
        }
    }
}

#[derive(Debug, Clone)]
pub struct FileTab {
    pub dir: PathBuf,
    pub history_back: Vec<PathBuf>,
    pub history_forward: Vec<PathBuf>,
}

#[derive(Debug, Clone)]
pub enum Message {
    NewTab,
    CloseTab(usize),
    SwitchTab(usize),
    OpenInNewTab(PathBuf),
    DuplicateTab(usize),
    CloseOtherTabs(usize),
    ToggleSplitView,
    FocusPane(ActivePane),
    CopyToOtherPane,
    MoveToOtherPane,
    NavigateTo(PathBuf),
    NavigateBack,
    NavigateForward,
    NavigateUp,
    Refresh,
    ToggleAddressEdit,
    AddressInputChanged(String),
    AddressSubmitted,
    SearchChanged(String),
    SelectIndex(usize),
    ActivateIndex(usize),
    ActivateSelected,
    SetViewMode(ViewMode),
    ToggleHidden,
    ToggleInspector,
    SortBy(SortColumn),
    CopySelected,
    CutSelected,
    PasteClipboard,
    PromptNewFolder,
    PromptNewFile,
    PromptRename,
    PromptPermanentDelete,
    ModalInputChanged(String),
    ConfirmModal,
    CancelModal,
    TrashSelected,
    RestoreSelectedFromTrash,
    EmptyTrash,
    ExtractSelectedArchive,
    CompressSelectedArchive(String),
    OpenTerminalHere,
    ToggleTerminalDrawer,
    TerminalInputChanged(String),
    ExecuteTerminalCommand,
    TerminalCommandFinished(Result<(Vec<String>, Vec<String>, bool), String>),
    ToggleBookmarkCurrent,
    RefreshDevices,
    DevicesRefreshed(Vec<corten_services::udisks2::BlockDevice>),
    MountDevice(String),
    DeviceMounted(Result<PathBuf, String>),
    UnmountDevice(String),
    DeviceUnmounted(Result<(), String>),
    AnalyzeWithAi(PathBuf),
    OpenContextMenu(usize),
    CloseContextMenu,
    ToggleBreadcrumbDropdown(PathBuf),
    CloseBreadcrumbDropdown,
    ToggleQuickLook,
    CloseQuickLook,
    NoOp,
    KeyEvent(keyboard::Key, keyboard::Modifiers),
}

pub fn main() -> iced::Result {
    tracing_subscriber::fmt::init();

    iced::application(
        "Corten Files",
        CortenFilesApp::update,
        CortenFilesApp::view,
    )
    .window(iced::window::Settings {
        size: iced::Size::new(1160.0, 740.0),
        min_size: Some(iced::Size::new(860.0, 540.0)),
        resizable: true,
        #[cfg(target_os = "linux")]
        platform_specific: iced::window::settings::PlatformSpecific {
            application_id: "corten-files".into(),
            ..Default::default()
        },
        ..Default::default()
    })
    .subscription(CortenFilesApp::subscription)
    .font(LUCIDE_FONT_BYTES)
    .font(NOTO_SANS_BYTES)
    .default_font(NOTO_SANS)
    .run()
}

pub struct CortenFilesApp {
    config: FilesConfig,
    tabs: Vec<FileTab>,
    active_tab: usize,
    split_view: bool,
    active_pane: ActivePane,
    right_pane: PaneState,
    current_dir: PathBuf,
    history_back: Vec<PathBuf>,
    history_forward: Vec<PathBuf>,
    places: Vec<SidebarPlace>,
    entries: Vec<FileEntry>,
    selected_index: Option<usize>,
    inspection: Option<EntryInspection>,
    search_query: String,
    editing_address: bool,
    address_input: String,
    clipboard: Option<ClipboardOp>,
    modal: Option<InlineModal>,
    status_message: String,
    devices: Vec<corten_services::udisks2::BlockDevice>,
    show_terminal_drawer: bool,
    terminal_input: String,
    terminal_log: Vec<String>,
    theme_mode: ThemeMode,
    active_patina: PatinaVariant,
    context_menu: Option<usize>,
    breadcrumb_dropdown: Option<PathBuf>,
    disk_usage: Option<(u64, u64, f32)>,
    show_quick_look: bool,
}

impl Default for CortenFilesApp {
    fn default() -> Self {
        let config = ConfigManager::<FilesConfig>::load_or_default("files")
            .map(|c| c.get().clone())
            .unwrap_or_default();

        let (active_patina, theme_mode) = corten_config::ConfigManager::<corten_config::AppearanceConfig>::load_or_default("appearance")
            .map(|c| {
                let p = PatinaVariant::from_code(&c.get().patina);
                let tm = match c.get().theme_mode {
                    corten_config::ThemeModeChoice::Light => ThemeMode::Light,
                    corten_config::ThemeModeChoice::Dark => ThemeMode::Dark,
                };
                (p, tm)
            })
            .unwrap_or((PatinaVariant::Rust, ThemeMode::Dark));

        // Allow passing initial directory as CLI argument (e.g. `corten-files ~/Pictures`)
        let initial_dir = std::env::args()
            .nth(1)
            .map(PathBuf::from)
            .filter(|p| p.exists() && p.is_dir())
            .or_else(|| {
                directories::BaseDirs::new().map(|b| b.home_dir().to_path_buf())
            })
            .unwrap_or_else(|| PathBuf::from("/"));

        let mut app = Self {
            config,
            tabs: vec![FileTab {
                dir: initial_dir.clone(),
                history_back: Vec::new(),
                history_forward: Vec::new(),
            }],
            active_tab: 0,
            split_view: false,
            active_pane: ActivePane::Left,
            right_pane: PaneState::new(initial_dir.clone()),
            current_dir: initial_dir.clone(),
            history_back: Vec::new(),
            history_forward: Vec::new(),
            places: standard_places(),
            entries: Vec::new(),
            selected_index: None,
            inspection: None,
            search_query: String::new(),
            editing_address: false,
            address_input: initial_dir.to_string_lossy().to_string(),
            clipboard: None,
            modal: None,
            status_message: "Hazır".to_string(),
            context_menu: None,
            breadcrumb_dropdown: None,
            disk_usage: None,
            show_quick_look: false,
            devices: Vec::new(),
            show_terminal_drawer: false,
            terminal_input: String::new(),
            terminal_log: Vec::new(),
            theme_mode,
            active_patina,
        };
        app.reload_entries();
        app.reload_devices();
        app
    }
}

impl CortenFilesApp {
    fn should_gatekeep(path: &std::path::Path) -> bool {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
        let is_executable_ext = matches!(ext.as_str(), "sh" | "bash" | "appimage" | "bin" | "run" | "py" | "elf");

        let in_downloads = if let Some(base) = directories::UserDirs::new() {
            if let Some(dl) = base.download_dir() {
                path.starts_with(dl)
            } else {
                false
            }
        } else {
            false
        };

        let is_exec_perm = {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(meta) = std::fs::metadata(path) {
                    meta.permissions().mode() & 0o111 != 0
                } else {
                    false
                }
            }
            #[cfg(not(unix))]
            {
                false
            }
        };

        (is_executable_ext || is_exec_perm) && (in_downloads || is_executable_ext)
    }

    fn compute_sha256(path: &std::path::Path) -> String {
        if let Ok(out) = std::process::Command::new("sha256sum").arg(path).output() {
            let text = String::from_utf8_lossy(&out.stdout);
            if let Some(hash) = text.split_whitespace().next() {
                return hash.to_string();
            }
        }
        if let Ok(meta) = std::fs::metadata(path) {
            return format!("boyut: {} B", meta.len());
        }
        "bilinmiyor".to_string()
    }

    fn save_config(&self) {
        if let Ok(mut mgr) = ConfigManager::<FilesConfig>::load_or_default("files") {
            let _ = mgr.update(self.config.clone());
        }
    }

    fn is_in_trash(&self) -> bool {
        self.current_dir == trash_files_dir()
    }

    fn reload_devices(&mut self) {
        if let Ok(devs) = corten_services::udisks2::list_devices() {
            self.devices = devs;
        }
    }

    fn reload_entries(&mut self) {
        self.places = standard_places();
        self.disk_usage = query_disk_usage(&self.current_dir);
        self.breadcrumb_dropdown = None;
        let prev_selected_path = self.selected_entry().map(|e| e.path.clone());

        match read_directory(
            &self.current_dir,
            self.config.show_hidden,
            &self.search_query,
            self.config.sort_column,
            self.config.sort_direction,
        ) {
            Ok(list) => {
                self.entries = list;
                self.selected_index = prev_selected_path
                    .and_then(|p| self.entries.iter().position(|e| e.path == p))
                    .or(if self.entries.is_empty() { None } else { Some(0) });
                self.refresh_inspection();
                self.address_input = self.current_dir.to_string_lossy().to_string();
                self.status_message = format!("{} öğe listeleniyor", self.entries.len());
            }
            Err(err) => {
                self.status_message = err;
            }
        }

        if self.split_view {
            let prev_right = self
                .right_pane
                .selected_index
                .and_then(|i| self.right_pane.entries.get(i).map(|e| e.path.clone()));
            if let Ok(rlist) = read_directory(
                &self.right_pane.dir,
                self.config.show_hidden,
                &self.right_pane.search_query,
                self.config.sort_column,
                self.config.sort_direction,
            ) {
                self.right_pane.entries = rlist;
                self.right_pane.selected_index = prev_right
                    .and_then(|p| self.right_pane.entries.iter().position(|e| e.path == p))
                    .or(if self.right_pane.entries.is_empty() {
                        None
                    } else {
                        Some(0)
                    });
                self.right_pane.address_input =
                    self.right_pane.dir.to_string_lossy().to_string();
            }
        }
    }

    fn refresh_inspection(&mut self) {
        self.inspection = self.selected_entry().map(inspect_entry);
    }

    fn selected_entry(&self) -> Option<&FileEntry> {
        self.selected_index.and_then(|idx| self.entries.get(idx))
    }

    fn sync_active_tab(&mut self) {
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.dir = self.current_dir.clone();
            tab.history_back = self.history_back.clone();
            tab.history_forward = self.history_forward.clone();
        }
    }

    fn load_active_tab(&mut self) {
        if let Some(tab) = self.tabs.get(self.active_tab).cloned() {
            self.current_dir = tab.dir;
            self.history_back = tab.history_back;
            self.history_forward = tab.history_forward;
            self.search_query.clear();
            self.editing_address = false;
            self.modal = None;
            self.reload_entries();
        }
    }

    fn navigate_to(&mut self, target: PathBuf, record_history: bool) {
        if !target.exists() || !target.is_dir() {
            self.status_message = format!("Klasör bulunamadı: {}", target.display());
            return;
        }
        if record_history && target != self.current_dir {
            self.history_back.push(self.current_dir.clone());
            self.history_forward.clear();
        }
        self.current_dir = target;
        self.sync_active_tab();
        self.search_query.clear();
        self.editing_address = false;
        self.modal = None;
        self.reload_entries();
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let key_sub = event::listen_with(|event, _status, _window| match event {
            Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) => {
                Some(Message::KeyEvent(key, modifiers))
            }
            _ => None,
        });
        let timer_sub = iced::time::every(std::time::Duration::from_secs(6)).map(|_| Message::RefreshDevices);
        Subscription::batch([key_sub, timer_sub])
    }

    pub fn update(&mut self, message: Message) -> iced::Task<Message> {
        match message {
            Message::ExecuteTerminalCommand => {
                let cmd_str = self.terminal_input.trim().to_string();
                if cmd_str.is_empty() {
                    return iced::Task::none();
                }
                self.terminal_log.push(format!("$ {}", cmd_str));
                let dir_clone = self.current_dir.clone();
                self.terminal_input.clear();
                iced::Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            let output = std::process::Command::new("sh")
                                .args(["-c", &cmd_str])
                                .current_dir(&dir_clone)
                                .output();
                            match output {
                                Ok(out) => {
                                    let mut stdout_lines = Vec::new();
                                    let stdout = String::from_utf8_lossy(&out.stdout);
                                    for line in stdout.lines().take(8) {
                                        stdout_lines.push(line.to_string());
                                    }
                                    let mut stderr_lines = Vec::new();
                                    let stderr = String::from_utf8_lossy(&out.stderr);
                                    for line in stderr.lines().take(4) {
                                        stderr_lines.push(format!("err: {}", line));
                                    }
                                    Ok((stdout_lines, stderr_lines, out.status.success()))
                                }
                                Err(e) => Err(format!("Komut hatası: {}", e)),
                            }
                        })
                        .await
                        .unwrap_or_else(|e| Err(format!("Task hatası: {}", e)))
                    },
                    Message::TerminalCommandFinished,
                )
            }
            Message::RefreshDevices => {
                iced::Task::perform(
                    async {
                        tokio::task::spawn_blocking(|| {
                            corten_services::udisks2::list_devices().unwrap_or_default()
                        })
                        .await
                        .unwrap_or_default()
                    },
                    Message::DevicesRefreshed,
                )
            }
            Message::MountDevice(obj_path) => {
                let obj_clone = obj_path.clone();
                iced::Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            corten_services::udisks2::mount_device(&obj_clone)
                                .map_err(|e| e.to_string())
                        })
                        .await
                        .unwrap_or_else(|e| Err(e.to_string()))
                    },
                    Message::DeviceMounted,
                )
            }
            Message::UnmountDevice(obj_path) => {
                let obj_clone = obj_path.clone();
                iced::Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            corten_services::udisks2::unmount_device(&obj_clone)
                                .map_err(|e| e.to_string())
                        })
                        .await
                        .unwrap_or_else(|e| Err(e.to_string()))
                    },
                    Message::DeviceUnmounted,
                )
            }
            other => {
                self.handle_sync_message(other);
                iced::Task::none()
            }
        }
    }

    fn handle_sync_message(&mut self, message: Message) {
        match message {
            Message::NewTab => {
                self.sync_active_tab();
                self.tabs.push(FileTab {
                    dir: self.current_dir.clone(),
                    history_back: Vec::new(),
                    history_forward: Vec::new(),
                });
                self.active_tab = self.tabs.len() - 1;
                self.load_active_tab();
                self.status_message = format!("Yeni sekme açıldı ({} sekme)", self.tabs.len());
            }
            Message::CloseTab(idx) => {
                if self.tabs.len() <= 1 {
                    return;
                }
                self.sync_active_tab();
                if idx < self.tabs.len() {
                    self.tabs.remove(idx);
                    if self.active_tab >= self.tabs.len() {
                        self.active_tab = self.tabs.len() - 1;
                    } else if idx < self.active_tab {
                        self.active_tab = self.active_tab.saturating_sub(1);
                    }
                    self.load_active_tab();
                }
            }
            Message::SwitchTab(idx) => {
                if idx < self.tabs.len() && idx != self.active_tab {
                    self.sync_active_tab();
                    self.active_tab = idx;
                    self.load_active_tab();
                }
            }
            Message::OpenInNewTab(path) => {
                self.sync_active_tab();
                self.tabs.push(FileTab {
                    dir: path.clone(),
                    history_back: Vec::new(),
                    history_forward: Vec::new(),
                });
                self.active_tab = self.tabs.len() - 1;
                self.load_active_tab();
                self.status_message = format!("Yeni sekmede açıldı: {}", path.display());
            }
            Message::DuplicateTab(idx) => {
                if let Some(tab) = self.tabs.get(idx).cloned() {
                    self.sync_active_tab();
                    self.tabs.push(tab);
                    self.active_tab = self.tabs.len() - 1;
                    self.load_active_tab();
                    self.status_message = format!("Sekme çoğaltıldı ({} sekme)", self.tabs.len());
                }
            }
            Message::CloseOtherTabs(idx) => {
                if idx < self.tabs.len() {
                    let keep = self.tabs[idx].clone();
                    self.tabs = vec![keep];
                    self.active_tab = 0;
                    self.load_active_tab();
                    self.status_message = "Diğer sekmeler kapatıldı".into();
                }
            }
            Message::ToggleSplitView => {
                self.split_view = !self.split_view;
                if self.split_view {
                    if self.right_pane.dir == self.current_dir {
                        if let Some(parent) = self.current_dir.parent() {
                            self.right_pane.dir = parent.to_path_buf();
                        }
                    }
                    self.reload_entries();
                    self.status_message = "Çift Panel (Dual-Pane) modu aktif (F3 / Tab)".into();
                } else {
                    self.active_pane = ActivePane::Left;
                    self.status_message = "Tek Panel görünümüne dönüldü".into();
                }
            }
            Message::FocusPane(pane) => {
                self.active_pane = pane;
            }
            Message::CopyToOtherPane => {
                if !self.split_view {
                    return;
                }
                let (src_entry, target_dir) = match self.active_pane {
                    ActivePane::Left => (self.selected_entry().cloned(), self.right_pane.dir.clone()),
                    ActivePane::Right => (
                        self.right_pane
                            .selected_index
                            .and_then(|i| self.right_pane.entries.get(i).cloned()),
                        self.current_dir.clone(),
                    ),
                };
                if let Some(entry) = src_entry {
                    match copy_entry_recursive(&entry.path, &target_dir) {
                        Ok(dst) => {
                            self.reload_entries();
                            self.status_message = format!("Diğer panele kopyalandı: {}", dst.display());
                        }
                        Err(e) => self.status_message = e,
                    }
                }
            }
            Message::MoveToOtherPane => {
                if !self.split_view {
                    return;
                }
                let (src_entry, target_dir) = match self.active_pane {
                    ActivePane::Left => (self.selected_entry().cloned(), self.right_pane.dir.clone()),
                    ActivePane::Right => (
                        self.right_pane
                            .selected_index
                            .and_then(|i| self.right_pane.entries.get(i).cloned()),
                        self.current_dir.clone(),
                    ),
                };
                if let Some(entry) = src_entry {
                    match move_entry(&entry.path, &target_dir) {
                        Ok(dst) => {
                            self.reload_entries();
                            self.status_message = format!("Diğer panele taşındı: {}", dst.display());
                        }
                        Err(e) => self.status_message = e,
                    }
                }
            }
            Message::NavigateTo(path) => {
                self.navigate_to(path, true);
            }
            Message::NavigateBack => {
                if let Some(prev) = self.history_back.pop() {
                    self.history_forward.push(self.current_dir.clone());
                    self.navigate_to(prev, false);
                }
            }
            Message::NavigateForward => {
                if let Some(next) = self.history_forward.pop() {
                    self.history_back.push(self.current_dir.clone());
                    self.navigate_to(next, false);
                }
            }
            Message::NavigateUp => {
                if let Some(parent) = self.current_dir.parent().map(|p| p.to_path_buf()) {
                    self.navigate_to(parent, true);
                }
            }
            Message::Refresh => {
                self.reload_entries();
            }
            Message::ToggleAddressEdit => {
                self.editing_address = !self.editing_address;
                self.address_input = self.current_dir.to_string_lossy().to_string();
            }
            Message::AddressInputChanged(val) => {
                self.address_input = val;
            }
            Message::AddressSubmitted => {
                let candidate = PathBuf::from(self.address_input.trim());
                self.editing_address = false;
                self.navigate_to(candidate, true);
            }
            Message::SearchChanged(query) => {
                self.search_query = query;
                self.reload_entries();
            }
            Message::SelectIndex(idx) => {
                self.context_menu = None;
                if idx < self.entries.len() {
                    if self.selected_index == Some(idx) {
                        // Second click on already-selected item activates it (opens folder/file)
                        self.handle_sync_message(Message::ActivateIndex(idx));
                        return;
                    }
                    self.selected_index = Some(idx);
                    self.refresh_inspection();
                }
            }
            Message::OpenContextMenu(idx) => {
                if idx < self.entries.len() {
                    self.selected_index = Some(idx);
                    self.refresh_inspection();
                    self.context_menu = Some(idx);
                }
            }
            Message::CloseContextMenu => {
                self.context_menu = None;
            }
            Message::ToggleBreadcrumbDropdown(path) => {
                if self.breadcrumb_dropdown.as_ref() == Some(&path) {
                    self.breadcrumb_dropdown = None;
                } else {
                    self.breadcrumb_dropdown = Some(path);
                }
            }
            Message::CloseBreadcrumbDropdown => {
                self.breadcrumb_dropdown = None;
            }
            Message::NoOp => {}
            Message::ActivateIndex(idx) => {
                if let Some(entry) = self.entries.get(idx).cloned() {
                    if entry.is_dir {
                        self.navigate_to(entry.path, true);
                    } else {
                        if Self::should_gatekeep(&entry.path) {
                            let sha = Self::compute_sha256(&entry.path);
                            self.modal = Some(InlineModal::SecurityGatekeeper {
                                path: entry.path,
                                sha256: sha,
                            });
                            return;
                        }
                        match open_with_system(&entry.path) {
                            Ok(()) => {
                                self.status_message = format!("Açıldı: {}", entry.name);
                            }
                            Err(e) => {
                                self.status_message = e;
                            }
                        }
                    }
                }
            }
            Message::ActivateSelected => {
                if let Some(idx) = self.selected_index {
                    self.handle_sync_message(Message::ActivateIndex(idx));
                }
            }
            Message::SetViewMode(mode) => {
                self.config.view_mode = mode;
                self.save_config();
            }
            Message::ToggleHidden => {
                self.config.show_hidden = !self.config.show_hidden;
                self.save_config();
                self.reload_entries();
            }
            Message::ToggleInspector => {
                self.config.show_inspector = !self.config.show_inspector;
                self.save_config();
            }
            Message::ToggleQuickLook => {
                if self.selected_entry().is_some() {
                    self.show_quick_look = !self.show_quick_look;
                    self.context_menu = None;
                }
            }
            Message::CloseQuickLook => {
                self.show_quick_look = false;
            }
            Message::SortBy(col) => {
                if self.config.sort_column == col {
                    self.config.sort_direction = self.config.sort_direction.toggle();
                } else {
                    self.config.sort_column = col;
                    self.config.sort_direction = SortDirection::Ascending;
                }
                self.save_config();
                self.reload_entries();
            }
            Message::CopySelected => {
                if let Some(entry) = self.selected_entry().cloned() {
                    self.clipboard = Some(ClipboardOp::Copy(entry.path));
                    self.status_message = format!("Panoya kopyalandı: {}", entry.name);
                }
            }
            Message::CutSelected => {
                if let Some(entry) = self.selected_entry().cloned() {
                    self.clipboard = Some(ClipboardOp::Cut(entry.path));
                    self.status_message = format!("Taşımak için kesildi: {}", entry.name);
                }
            }
            Message::PasteClipboard => {
                let Some(op) = self.clipboard.clone() else {
                    self.status_message = "Pano boş".into();
                    return;
                };
                match op {
                    ClipboardOp::Copy(src) => match copy_entry_recursive(&src, &self.current_dir) {
                        Ok(dst) => {
                            self.reload_entries();
                            self.status_message = format!("Yapıştırıldı: {}", dst.display());
                        }
                        Err(e) => self.status_message = e,
                    },
                    ClipboardOp::Cut(src) => match move_entry(&src, &self.current_dir) {
                        Ok(dst) => {
                            self.clipboard = None;
                            self.reload_entries();
                            self.status_message = format!("Taşındı: {}", dst.display());
                        }
                        Err(e) => self.status_message = e,
                    },
                }
            }
            Message::PromptNewFolder => {
                self.modal = Some(InlineModal::NewFolder("Yeni Klasör".into()));
            }
            Message::PromptNewFile => {
                self.modal = Some(InlineModal::NewFile("yeni_dosya.txt".into()));
            }
            Message::PromptRename => {
                if let Some(entry) = self.selected_entry() {
                    self.modal = Some(InlineModal::Rename(entry.path.clone(), entry.name.clone()));
                }
            }
            Message::PromptPermanentDelete => {
                if let Some(entry) = self.selected_entry() {
                    self.modal = Some(InlineModal::ConfirmPermanentDelete(entry.path.clone()));
                }
            }
            Message::ModalInputChanged(val) => match &mut self.modal {
                Some(InlineModal::NewFolder(ref mut s))
                | Some(InlineModal::NewFile(ref mut s))
                | Some(InlineModal::Rename(_, ref mut s)) => {
                    *s = val;
                }
                _ => {}
            },
            Message::ConfirmModal => {
                let Some(modal) = self.modal.take() else {
                    return;
                };
                match modal {
                    InlineModal::NewFolder(name) => match create_folder(&self.current_dir, &name) {
                        Ok(created) => {
                            self.reload_entries();
                            self.selected_index =
                                self.entries.iter().position(|e| e.path == created);
                            self.refresh_inspection();
                            self.status_message = format!("Klasör oluşturuldu: {name}");
                        }
                        Err(e) => self.status_message = e,
                    },
                    InlineModal::NewFile(name) => match create_file(&self.current_dir, &name) {
                        Ok(created) => {
                            self.reload_entries();
                            self.selected_index =
                                self.entries.iter().position(|e| e.path == created);
                            self.refresh_inspection();
                            self.status_message = format!("Dosya oluşturuldu: {name}");
                        }
                        Err(e) => self.status_message = e,
                    },
                    InlineModal::Rename(src, new_name) => match rename_entry(&src, &new_name) {
                        Ok(renamed) => {
                            self.reload_entries();
                            self.selected_index =
                                self.entries.iter().position(|e| e.path == renamed);
                            self.refresh_inspection();
                            self.status_message = format!("Yeniden adlandırıldı: {new_name}");
                        }
                        Err(e) => self.status_message = e,
                    },
                    InlineModal::ConfirmPermanentDelete(path) => match delete_permanently(&path) {
                        Ok(()) => {
                            self.reload_entries();
                            self.status_message = "Kalıcı olarak silindi".into();
                        }
                        Err(e) => self.status_message = e,
                    },
                    InlineModal::SecurityGatekeeper { path, .. } => {
                        match open_with_system(&path) {
                            Ok(()) => {
                                self.status_message = format!("Güvenle çalıştırıldı: {}", path.display());
                            }
                            Err(e) => {
                                self.status_message = e;
                            }
                        }
                    }
                }
            }
            Message::AnalyzeWithAi(path) => {
                let _ = std::process::Command::new("corten-ai")
                    .arg("--audit")
                    .arg(&path)
                    .spawn();
                self.status_message = format!("Corten AI güvenlik denetimi başlatıldı: {}", path.display());
            }
            Message::CancelModal => {
                self.modal = None;
            }
            Message::TrashSelected => {
                let Some(entry) = self.selected_entry().cloned() else {
                    return;
                };
                if self.is_in_trash() {
                    self.modal = Some(InlineModal::ConfirmPermanentDelete(entry.path));
                } else {
                    match move_to_trash(&entry.path) {
                        Ok(_) => {
                            self.reload_entries();
                            self.status_message = format!("Çöp kutusuna taşındı: {}", entry.name);
                        }
                        Err(e) => self.status_message = e,
                    }
                }
            }
            Message::RestoreSelectedFromTrash => {
                let Some(entry) = self.selected_entry().cloned() else {
                    return;
                };
                match restore_from_trash(&entry.path) {
                    Ok(restored) => {
                        self.reload_entries();
                        self.status_message =
                            format!("Geri yüklendi: {}", restored.to_string_lossy());
                    }
                    Err(e) => self.status_message = e,
                }
            }
            Message::EmptyTrash => match empty_trash() {
                Ok(()) => {
                    self.reload_entries();
                    self.status_message = "Çöp kutusu boşaltıldı".into();
                }
                Err(e) => self.status_message = e,
            },
            Message::ExtractSelectedArchive => {
                let Some(entry) = self.selected_entry().cloned() else {
                    return;
                };
                match extract_archive(&entry.path, &self.current_dir) {
                    Ok(out_dir) => {
                        self.reload_entries();
                        self.status_message =
                            format!("Arşiv çıkartıldı: {}", out_dir.to_string_lossy());
                    }
                    Err(e) => self.status_message = e,
                }
            }
            Message::CompressSelectedArchive(fmt) => {
                let Some(entry) = self.selected_entry().cloned() else {
                    return;
                };
                match create_archive(&entry.path, &fmt) {
                    Ok(dest) => {
                        let name = dest
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_default();
                        self.status_message = format!("Arşiv oluşturuldu: {}", name);
                        self.reload_entries();
                    }
                    Err(e) => self.status_message = format!("Arşivleme hatası: {}", e),
                }
            }
            Message::OpenTerminalHere => match spawn_terminal_in(&self.current_dir) {
                Ok(()) => {
                    self.status_message =
                        format!("Terminal açıldı: {}", self.current_dir.display());
                }
                Err(e) => self.status_message = e,
            },
            Message::ToggleTerminalDrawer => {
                self.show_terminal_drawer = !self.show_terminal_drawer;
            }
            Message::TerminalInputChanged(input) => {
                self.terminal_input = input;
            }
            Message::TerminalCommandFinished(res) => {
                match res {
                    Ok((stdout, stderr, success)) => {
                        for line in stdout {
                            self.terminal_log.push(line);
                        }
                        for line in stderr {
                            self.terminal_log.push(line);
                        }
                        if success {
                            self.reload_entries();
                        }
                    }
                    Err(err) => {
                        self.terminal_log.push(err);
                    }
                }
                if self.terminal_log.len() > 25 {
                    let drain_count = self.terminal_log.len() - 25;
                    self.terminal_log.drain(0..drain_count);
                }
            }
            Message::ToggleBookmarkCurrent => {
                let current_str = self.current_dir.to_string_lossy().to_string();
                if let Some(pos) = self
                    .config
                    .bookmarks
                    .iter()
                    .position(|b| b == &current_str)
                {
                    self.config.bookmarks.remove(pos);
                    self.status_message = "Yer imlerinden kaldırıldı".into();
                } else {
                    self.config.bookmarks.push(current_str);
                    self.status_message = "Yer imlerine eklendi".into();
                }
                self.save_config();
            }
            Message::DevicesRefreshed(devs) => {
                self.devices = devs;
            }
            Message::DeviceMounted(res) => {
                match res {
                    Ok(mount_path) => {
                        self.status_message = format!("Aygıt bağlandı: {}", mount_path.display());
                        self.reload_devices();
                        self.navigate_to(mount_path, true);
                    }
                    Err(e) => {
                        self.status_message = format!("Bağlama hatası: {e}");
                    }
                }
            }
            Message::DeviceUnmounted(res) => {
                match res {
                    Ok(()) => {
                        self.status_message = "Aygıt güvenle çıkarıldı".into();
                        self.reload_devices();
                        if let Some(home) = directories::BaseDirs::new().map(|b| b.home_dir().to_path_buf()) {
                            if !self.current_dir.exists() {
                                self.navigate_to(home, true);
                            }
                        }
                    }
                    Err(e) => {
                        self.status_message = format!("Çıkarma hatası: {e}");
                    }
                }
            }
            Message::ExecuteTerminalCommand
            | Message::RefreshDevices
            | Message::MountDevice(_)
            | Message::UnmountDevice(_) => {
                // Handled in async task above
            }
            Message::KeyEvent(key, modifiers) => {
                if self.show_quick_look
                    && key == keyboard::Key::Named(keyboard::key::Named::Escape)
                {
                    self.show_quick_look = false;
                    return;
                }
                if self.context_menu.is_some() {
                    if key == keyboard::Key::Named(keyboard::key::Named::Escape) {
                        self.context_menu = None;
                    }
                    return;
                }
                if self.breadcrumb_dropdown.is_some() {
                    if key == keyboard::Key::Named(keyboard::key::Named::Escape) {
                        self.breadcrumb_dropdown = None;
                    }
                    return;
                }
                if self.modal.is_some() {
                    if key == keyboard::Key::Named(keyboard::key::Named::Escape) {
                        self.modal = None;
                    } else if key == keyboard::Key::Named(keyboard::key::Named::Enter) {
                        self.handle_sync_message(Message::ConfirmModal);
                    }
                    return;
                }
                if self.editing_address {
                    if key == keyboard::Key::Named(keyboard::key::Named::Escape) {
                        self.editing_address = false;
                    }
                    return;
                }
                if !self.search_query.is_empty() && key == keyboard::Key::Named(keyboard::key::Named::Escape) {
                    self.handle_sync_message(Message::SearchChanged(String::new()));
                    return;
                }

                match key.as_ref() {
                    keyboard::Key::Named(keyboard::key::Named::ArrowDown) => {
                        if !self.entries.is_empty() {
                            let next = self
                                .selected_index
                                .map(|i| (i + 1).min(self.entries.len() - 1))
                                .unwrap_or(0);
                            self.selected_index = Some(next);
                            self.refresh_inspection();
                        }
                    }
                    keyboard::Key::Named(keyboard::key::Named::ArrowUp) if modifiers.alt() => {
                        self.handle_sync_message(Message::NavigateUp);
                    }
                    keyboard::Key::Named(keyboard::key::Named::ArrowUp) => {
                        if !self.entries.is_empty() {
                            let prev = self
                                .selected_index
                                .map(|i| i.saturating_sub(1))
                                .unwrap_or(0);
                            self.selected_index = Some(prev);
                            self.refresh_inspection();
                        }
                    }
                    keyboard::Key::Named(keyboard::key::Named::Enter) => {
                        self.handle_sync_message(Message::ActivateSelected);
                    }
                    keyboard::Key::Named(keyboard::key::Named::Space)
                        if self.search_query.is_empty() =>
                    {
                        self.handle_sync_message(Message::ToggleQuickLook);
                    }
                    keyboard::Key::Character("i" | "I") if modifiers.control() => {
                        self.handle_sync_message(Message::ToggleInspector);
                    }
                    keyboard::Key::Named(keyboard::key::Named::Backspace)
                        if self.search_query.is_empty() =>
                    {
                        self.handle_sync_message(Message::NavigateUp);
                    }
                    keyboard::Key::Named(keyboard::key::Named::ArrowLeft) if modifiers.alt() => {
                        self.handle_sync_message(Message::NavigateBack);
                    }
                    keyboard::Key::Named(keyboard::key::Named::ArrowRight) if modifiers.alt() => {
                        self.handle_sync_message(Message::NavigateForward);
                    }
                    keyboard::Key::Named(keyboard::key::Named::ArrowLeft)
                        if self.config.view_mode == ViewMode::Columns
                            && self.search_query.is_empty() =>
                    {
                        self.handle_sync_message(Message::NavigateUp);
                    }
                    keyboard::Key::Named(keyboard::key::Named::ArrowRight)
                        if self.config.view_mode == ViewMode::Columns
                            && self.search_query.is_empty() =>
                    {
                        if let Some(entry) = self.selected_entry() {
                            if entry.is_dir {
                                self.handle_sync_message(Message::ActivateSelected);
                            }
                        }
                    }
                    keyboard::Key::Character("1") if modifiers.control() => {
                        self.handle_sync_message(Message::SetViewMode(ViewMode::List));
                    }
                    keyboard::Key::Character("2") if modifiers.control() => {
                        self.handle_sync_message(Message::SetViewMode(ViewMode::Grid));
                    }
                    keyboard::Key::Character("3") if modifiers.control() => {
                        self.handle_sync_message(Message::SetViewMode(ViewMode::Columns));
                    }
                    keyboard::Key::Named(keyboard::key::Named::F2) => {
                        self.handle_sync_message(Message::PromptRename);
                    }
                    keyboard::Key::Named(keyboard::key::Named::F3) => {
                        self.handle_sync_message(Message::ToggleSplitView);
                    }
                    keyboard::Key::Named(keyboard::key::Named::Tab) if self.split_view => {
                        let next = match self.active_pane {
                            ActivePane::Left => ActivePane::Right,
                            ActivePane::Right => ActivePane::Left,
                        };
                        self.handle_sync_message(Message::FocusPane(next));
                    }
                    keyboard::Key::Named(keyboard::key::Named::F5) if self.split_view => {
                        self.handle_sync_message(Message::CopyToOtherPane);
                    }
                    keyboard::Key::Named(keyboard::key::Named::F6) if self.split_view => {
                        self.handle_sync_message(Message::MoveToOtherPane);
                    }
                    keyboard::Key::Named(keyboard::key::Named::F4) => {
                        if modifiers.shift() {
                            self.handle_sync_message(Message::OpenTerminalHere);
                        } else {
                            self.handle_sync_message(Message::ToggleTerminalDrawer);
                        }
                    }
                    keyboard::Key::Character("`") if modifiers.control() => {
                        self.handle_sync_message(Message::ToggleTerminalDrawer);
                    }
                    keyboard::Key::Named(keyboard::key::Named::F5) => {
                        self.handle_sync_message(Message::Refresh);
                    }
                    keyboard::Key::Named(keyboard::key::Named::Delete) => {
                        if modifiers.shift() {
                            self.handle_sync_message(Message::PromptPermanentDelete);
                        } else {
                            self.handle_sync_message(Message::TrashSelected);
                        }
                    }
                    keyboard::Key::Character("t" | "T") if modifiers.control() => {
                        self.handle_sync_message(Message::NewTab);
                    }
                    keyboard::Key::Character("w" | "W") if modifiers.control() => {
                        self.handle_sync_message(Message::CloseTab(self.active_tab));
                    }
                    keyboard::Key::Named(keyboard::key::Named::Tab) if modifiers.control() => {
                        let next_tab = (self.active_tab + 1) % self.tabs.len().max(1);
                        self.handle_sync_message(Message::SwitchTab(next_tab));
                    }
                    keyboard::Key::Character("f" | "F") if modifiers.control() => {
                        self.handle_sync_message(Message::SearchChanged(String::new()));
                    }
                    keyboard::Key::Character("h" | "H") if modifiers.control() => {
                        self.handle_sync_message(Message::ToggleHidden);
                    }
                    keyboard::Key::Character("l" | "L") if modifiers.control() => {
                        self.handle_sync_message(Message::ToggleAddressEdit);
                    }
                    keyboard::Key::Character("c" | "C") if modifiers.control() => {
                        self.handle_sync_message(Message::CopySelected);
                    }
                    keyboard::Key::Character("x" | "X") if modifiers.control() => {
                        self.handle_sync_message(Message::CutSelected);
                    }
                    keyboard::Key::Character("v" | "V") if modifiers.control() => {
                        self.handle_sync_message(Message::PasteClipboard);
                    }
                    keyboard::Key::Character("n" | "N")
                        if modifiers.control() && modifiers.shift() =>
                    {
                        self.handle_sync_message(Message::PromptNewFolder);
                    }
                    _ => {}
                }
            }
        }
    }

    pub fn view(&self) -> Element<'_, Message> {
        let palette = Palette::for_mode(self.theme_mode).with_patina(self.active_patina);

        let tab_bar = self.view_tab_bar(palette);
        let top_bar = self.view_top_bar(palette);

        let explorer_col: Element<'_, Message> = if self.split_view {
            row![
                container(self.view_explorer_pane(palette))
                    .width(Length::FillPortion(1))
                    .height(Length::Fill)
                    .style(move |_| {
                        let is_active = self.active_pane == ActivePane::Left;
                        container::Style {
                            border: Border {
                                color: if is_active {
                                    palette.primary.to_iced()
                                } else {
                                    Color::from_rgba(1.0, 1.0, 1.0, 0.05)
                                },
                                width: if is_active { 1.5 } else { 1.0 },
                                radius: border::Radius::from(Radius::SM),
                            },
                            ..Default::default()
                        }
                    }),
                container(self.view_right_explorer_pane(palette))
                    .width(Length::FillPortion(1))
                    .height(Length::Fill)
                    .style(move |_| {
                        let is_active = self.active_pane == ActivePane::Right;
                        container::Style {
                            border: Border {
                                color: if is_active {
                                    palette.primary.to_iced()
                                } else {
                                    Color::from_rgba(1.0, 1.0, 1.0, 0.05)
                                },
                                width: if is_active { 1.5 } else { 1.0 },
                                radius: border::Radius::from(Radius::SM),
                            },
                            ..Default::default()
                        }
                    }),
            ]
            .spacing(Spacing::XS)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
        } else {
            self.view_explorer_pane(palette)
        };

        let main_body = row![
            self.view_sidebar(palette),
            explorer_col,
        ]
        .spacing(0)
        .height(Length::Fill);

        let full_row = if self.config.show_inspector && self.config.view_mode != ViewMode::Columns {
            main_body.push(self.view_inspector_pane(palette))
        } else {
            main_body
        };

        let mut center_col = column![tab_bar, top_bar].spacing(2);
        if let Some(ref dropdown_dir) = self.breadcrumb_dropdown {
            center_col = center_col.push(self.view_breadcrumb_dropdown(dropdown_dir, palette));
        }
        if let Some(ref modal) = self.modal {
            center_col = center_col.push(self.view_inline_modal(modal, palette));
        }
        center_col = center_col.push(full_row);

        if self.show_terminal_drawer {
            center_col = center_col.push(self.view_terminal_drawer(palette));
        }

        center_col = center_col.push(self.view_status_bar(palette));

        let base_view = container(center_col)
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(Spacing::XS)
            .style(move |_| container::Style {
                background: Some(palette.background.to_iced().into()),
                text_color: Some(palette.text_primary.to_iced()),
                ..Default::default()
            });

        if self.show_quick_look {
            let ql_card = self.view_quick_look(palette);
            let inner_overlay = mouse_area(ql_card).on_press(Message::NoOp);
            let dismiss_overlay = mouse_area(
                container(inner_overlay)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .center_x(Length::Fill)
                    .center_y(Length::Fill)
                    .style(|_| container::Style {
                        background: Some(iced::Background::Color(Color::from_rgba(
                            0.02, 0.03, 0.05, 0.72,
                        ))),
                        ..Default::default()
                    }),
            )
            .on_press(Message::CloseQuickLook)
            .on_right_press(Message::CloseQuickLook);

            stack![base_view, dismiss_overlay].into()
        } else if let Some(idx) = self.context_menu {
            let menu_card = self.view_context_menu(idx, palette);
            let inner_overlay = mouse_area(menu_card).on_press(Message::NoOp);
            let dismiss_overlay = mouse_area(
                container(inner_overlay)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .center_x(Length::Fill)
                    .center_y(Length::Fill),
            )
            .on_press(Message::CloseContextMenu)
            .on_right_press(Message::CloseContextMenu);

            stack![base_view, dismiss_overlay].into()
        } else {
            base_view.into()
        }
    }

    fn view_quick_look(&self, palette: Palette) -> Element<'_, Message> {
        let Some(entry) = self.selected_entry() else {
            return column![].into();
        };
        let icon_char = Self::entry_icon(entry.kind);
        let icon_color = Self::entry_color(entry.kind, palette);
        let size_str = if entry.is_dir {
            "Klasör".to_string()
        } else {
            format_size(entry.size)
        };
        let modified_str = format_relative_time(entry.modified_secs);

        let header = row![
            container(
                text(icon_char)
                    .font(LUCIDE_FONT)
                    .size(18)
                    .color(icon_color),
            )
            .padding([6, 8])
            .style(move |_| container::Style {
                background: Some(iced::Background::Color(Color::from_rgba(
                    icon_color.r,
                    icon_color.g,
                    icon_color.b,
                    0.14,
                ))),
                border: Border {
                    color: Color::from_rgba(icon_color.r, icon_color.g, icon_color.b, 0.30),
                    width: 1.0,
                    radius: border::Radius::from(Radius::SM),
                },
                ..Default::default()
            }),
            column![
                row![
                    text(entry.name.clone())
                        .size(Typography::TITLE_SM)
                        .color(palette.text_primary.to_iced()),
                    container(
                        text(entry.kind.badge())
                            .size(10)
                            .color(palette.primary.to_iced()),
                    )
                    .padding([2, 6])
                    .style(move |_| container::Style {
                        background: Some(iced::Background::Color(Color::from_rgba(
                            palette.primary.r,
                            palette.primary.g,
                            palette.primary.b,
                            0.14,
                        ))),
                        border: Border {
                            radius: border::Radius::from(Radius::FULL),
                            ..Default::default()
                        },
                        ..Default::default()
                    }),
                ]
                .spacing(Spacing::XS)
                .align_y(Alignment::Center),
                text(format!("{} • {}", size_str, modified_str))
                    .size(Typography::CAPTION)
                    .color(palette.text_muted.to_iced()),
            ]
            .spacing(2),
            horizontal_space(),
            Self::accent_button("Uygulama İle Aç (Enter)", Message::ActivateSelected, palette),
            Self::small_button("✕ Kapat (Space)", "", Message::CloseQuickLook, palette),
        ]
        .spacing(Spacing::SM)
        .align_y(Alignment::Center);

        let stage_body: Element<'_, Message> = if let Some(img_path) =
            fs_ops::get_image_preview_path(&entry.path)
        {
            container(
                iced::widget::image(iced::widget::image::Handle::from_path(img_path))
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .content_fit(iced::ContentFit::Contain),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(Spacing::MD)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .style(|_| container::Style {
                background: Some(iced::Background::Color(Color::from_rgba(
                    0.04, 0.05, 0.07, 0.90,
                ))),
                border: Border {
                    color: Color::from_rgba(1.0, 1.0, 1.0, 0.07),
                    width: 1.0,
                    radius: border::Radius::from(Radius::MD),
                },
                ..Default::default()
            })
            .into()
        } else if entry.is_dir {
            let children = read_directory(
                &entry.path,
                self.config.show_hidden,
                "",
                SortColumn::Name,
                SortDirection::Ascending,
            )
            .unwrap_or_default();
            let mut list_col = column![
                text(format!("KLASÖR İÇERİĞİ ({} öğe)", children.len()))
                    .size(Typography::CAPTION)
                    .color(palette.text_muted.to_iced()),
            ]
            .spacing(6);
            for child in children.iter().take(24) {
                let c_icon = Self::entry_icon(child.kind);
                let c_color = Self::entry_color(child.kind, palette);
                list_col = list_col.push(
                    row![
                        text(c_icon).font(LUCIDE_FONT).size(14).color(c_color),
                        text(child.name.clone())
                            .size(Typography::BODY_SM)
                            .color(palette.text_primary.to_iced())
                            .width(Length::Fill),
                        text(if child.is_dir {
                            "Klasör".to_string()
                        } else {
                            format_size(child.size)
                        })
                        .size(Typography::CAPTION)
                        .color(palette.text_muted.to_iced()),
                    ]
                    .spacing(Spacing::SM)
                    .align_y(Alignment::Center),
                );
            }
            container(scrollable(list_col).height(Length::Fill))
                .width(Length::Fill)
                .height(Length::Fill)
                .padding(Spacing::MD)
                .style(move |_| input_style(palette))
                .into()
        } else if let Some(preview) =
            self.inspection.as_ref().and_then(|i| i.text_preview.as_ref())
        {
            let mut numbered_lines = column![].spacing(2);
            for (line_idx, line_str) in preview.lines().take(80).enumerate() {
                numbered_lines = numbered_lines.push(
                    row![
                        text(format!("{:>3} │", line_idx + 1))
                            .font(iced::Font::MONOSPACE)
                            .size(11.5)
                            .color(Color::from_rgba(1.0, 1.0, 1.0, 0.30))
                            .width(Length::Fixed(44.0)),
                        text(line_str.to_string())
                            .font(iced::Font::MONOSPACE)
                            .size(12.0)
                            .color(palette.text_primary.to_iced()),
                    ]
                    .spacing(8),
                );
            }
            container(scrollable(numbered_lines).height(Length::Fill))
                .width(Length::Fill)
                .height(Length::Fill)
                .padding(Spacing::MD)
                .style(|_| container::Style {
                    background: Some(iced::Background::Color(Color::from_rgba(
                        0.05, 0.06, 0.08, 0.94,
                    ))),
                    border: Border {
                        color: Color::from_rgba(1.0, 1.0, 1.0, 0.08),
                        width: 1.0,
                        radius: border::Radius::from(Radius::MD),
                    },
                    ..Default::default()
                })
                .into()
        } else {
            let perms = self
                .inspection
                .as_ref()
                .map(|i| i.permissions.as_str())
                .unwrap_or("---");
            container(
                column![
                    text(icon_char).font(LUCIDE_FONT).size(48).color(icon_color),
                    text(entry.name.clone())
                        .size(Typography::TITLE)
                        .color(palette.text_primary.to_iced()),
                    Self::meta_row("Tür", entry.kind.label(), palette),
                    Self::meta_row("Boyut", &size_str, palette),
                    Self::meta_row("İzinler", perms, palette),
                    Self::meta_row("Değiştirilme", &modified_str, palette),
                ]
                .spacing(Spacing::SM)
                .align_x(Alignment::Center),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .style(move |_| input_style(palette))
            .into()
        };

        let footer = row![
            text(entry.path.to_string_lossy().to_string())
                .size(Typography::CAPTION)
                .color(palette.text_muted.to_iced()),
            horizontal_space(),
            text("↑/↓ Dosya Değiştir • Space / Esc Kapat • Enter Aç")
                .size(Typography::CAPTION)
                .color(palette.primary.to_iced()),
        ]
        .align_y(Alignment::Center);

        container(
            column![header, stage_body, footer]
                .spacing(Spacing::SM)
                .height(Length::Fill),
        )
        .width(Length::Fixed(760.0))
        .height(Length::Fixed(520.0))
        .padding(Spacing::MD)
        .style(move |_| corten_theme::iced_styles::acrylic_glass_style(palette, Radius::LG))
        .into()
    }

    fn context_menu_item(
        icon: char,
        label: &'static str,
        shortcut: &'static str,
        is_destructive: bool,
        msg: Message,
        palette: Palette,
    ) -> Element<'static, Message> {
        let icon_color = if is_destructive {
            palette.danger.to_iced()
        } else {
            palette.primary.to_iced()
        };
        let label_color = if is_destructive {
            palette.danger.to_iced()
        } else {
            palette.text_primary.to_iced()
        };

        let mut r = row![
            text(icon).font(LUCIDE_FONT).size(14).color(icon_color),
            text(label)
                .size(Typography::BODY_SM)
                .color(label_color)
                .width(Length::Fill),
        ]
        .spacing(8)
        .align_y(Alignment::Center);

        if !shortcut.is_empty() {
            r = r.push(
                container(
                    text(shortcut)
                        .font(iced::Font::MONOSPACE)
                        .size(10.0)
                        .color(palette.text_muted.to_iced()),
                )
                .padding([2, 5])
                .style(|_| container::Style {
                    background: Some(iced::Background::Color(Color::from_rgba(
                        1.0, 1.0, 1.0, 0.05,
                    ))),
                    border: Border {
                        color: Color::from_rgba(1.0, 1.0, 1.0, 0.08),
                        width: 1.0,
                        radius: border::Radius::from(Radius::SM),
                    },
                    ..Default::default()
                }),
            );
        }

        button(r)
            .width(Length::Fill)
            .padding([6, 10])
            .on_press(msg)
            .style(move |_, status| {
                let bg = match status {
                    button::Status::Hovered => {
                        if is_destructive {
                            Color::from_rgba(
                                palette.danger.r,
                                palette.danger.g,
                                palette.danger.b,
                                0.16,
                            )
                        } else {
                            Color::from_rgba(1.0, 1.0, 1.0, 0.08)
                        }
                    }
                    button::Status::Pressed => Color::from_rgba(1.0, 1.0, 1.0, 0.12),
                    _ => Color::TRANSPARENT,
                };
                button::Style {
                    background: Some(iced::Background::Color(bg)),
                    border: Border {
                        radius: border::Radius::from(Radius::SM),
                        ..Default::default()
                    },
                    ..Default::default()
                }
            })
            .into()
    }

    fn ribbon_action_btn(
        icon: char,
        label: &'static str,
        is_destructive: bool,
        msg: Message,
        palette: Palette,
    ) -> Element<'static, Message> {
        let accent = if is_destructive {
            palette.danger.to_iced()
        } else {
            palette.primary.to_iced()
        };
        button(
            column![
                text(icon).font(LUCIDE_FONT).size(14).color(accent),
                text(label).size(9.5).color(if is_destructive {
                    accent
                } else {
                    palette.text_primary.to_iced()
                }),
            ]
            .spacing(2)
            .align_x(Alignment::Center),
        )
        .width(Length::FillPortion(1))
        .padding([5, 4])
        .on_press(msg)
        .style(move |_, status| {
            let bg = match status {
                button::Status::Hovered => Color::from_rgba(accent.r, accent.g, accent.b, 0.18),
                button::Status::Pressed => Color::from_rgba(accent.r, accent.g, accent.b, 0.26),
                _ => Color::from_rgba(1.0, 1.0, 1.0, 0.04),
            };
            button::Style {
                background: Some(iced::Background::Color(bg)),
                border: Border {
                    color: Color::from_rgba(1.0, 1.0, 1.0, 0.08),
                    width: 1.0,
                    radius: border::Radius::from(Radius::SM),
                },
                ..Default::default()
            }
        })
        .into()
    }

    fn view_context_menu(&self, idx: usize, palette: Palette) -> Element<'_, Message> {
        let Some(entry) = self.entries.get(idx) else {
            return column![].into();
        };

        let is_archive = matches!(
            entry.path.extension().and_then(|s| s.to_str()).unwrap_or(""),
            "zip" | "tar" | "gz" | "xz" | "7z" | "zst"
        );

        let icon_char = Self::entry_icon(entry.kind);
        let icon_color = Self::entry_accent_color(entry, palette);

        let header = row![
            text(icon_char)
                .font(LUCIDE_FONT)
                .size(16)
                .color(icon_color),
            text(entry.name.clone())
                .size(Typography::BODY_SM)
                .color(palette.text_primary.to_iced()),
            horizontal_space(),
            button(text("✕").size(12).color(palette.text_muted.to_iced()))
                .padding([2, 6])
                .style(|_, _| button::Style::default())
                .on_press(Message::CloseContextMenu),
        ]
        .spacing(Spacing::XS)
        .align_y(Alignment::Center);

        // Quick Action Ribbon (Cut / Copy / Rename / Trash)
        let quick_ribbon = row![
            Self::ribbon_action_btn(Icon::CLIPBOARD, "Kes", false, Message::CutSelected, palette),
            Self::ribbon_action_btn(
                Icon::CLIPBOARD,
                "Kopyala",
                false,
                Message::CopySelected,
                palette,
            ),
            Self::ribbon_action_btn(
                Icon::CODE,
                "Adlandır",
                false,
                Message::PromptRename,
                palette,
            ),
            Self::ribbon_action_btn(Icon::TRASH, "Sil", true, Message::TrashSelected, palette),
        ]
        .spacing(4)
        .width(Length::Fill);

        let divider = || {
            container(row![])
                .width(Length::Fill)
                .height(Length::Fixed(1.0))
                .style(|_| container::Style {
                    background: Some(iced::Background::Color(Color::from_rgba(
                        1.0, 1.0, 1.0, 0.08,
                    ))),
                    ..Default::default()
                })
        };

        let mut items = column![header, quick_ribbon, divider()].spacing(4);

        // 1. Open & Quick Look
        items = items.push(Self::context_menu_item(
            Icon::CHEVRON_RIGHT,
            "Aç",
            "Enter",
            false,
            Message::ActivateIndex(idx),
            palette,
        ));
        items = items.push(Self::context_menu_item(
            Icon::SEARCH,
            "Hızlı Bakış",
            "Space",
            false,
            Message::ToggleQuickLook,
            palette,
        ));

        // 2. Open in New Tab (if directory)
        if entry.is_dir {
            items = items.push(Self::context_menu_item(
                Icon::FOLDER,
                "Yeni Sekmede Aç",
                "Ctrl+T",
                false,
                Message::OpenInNewTab(entry.path.clone()),
                palette,
            ));
        }

        // 3. Extract / Compress
        if is_archive {
            items = items.push(Self::context_menu_item(
                Icon::DOWNLOAD,
                "Arşivi Buraya Çıkar",
                "",
                false,
                Message::ExtractSelectedArchive,
                palette,
            ));
        } else {
            items = items.push(Self::context_menu_item(
                Icon::CLIPBOARD,
                "ZIP Olarak Arşivle",
                "",
                false,
                Message::CompressSelectedArchive("zip".into()),
                palette,
            ));
        }

        items = items.push(divider());

        // 4. Terminal & AI Analysis
        items = items.push(Self::context_menu_item(
            Icon::TERMINAL,
            "Terminalde Aç",
            "F4",
            false,
            Message::OpenTerminalHere,
            palette,
        ));
        items = items.push(Self::context_menu_item(
            Icon::SHIELD,
            "Corten AI ile İncele",
            "",
            false,
            Message::AnalyzeWithAi(entry.path.clone()),
            palette,
        ));

        items = items.push(divider());

        // 5. Destructive Delete at bottom
        items = items.push(Self::context_menu_item(
            Icon::TRASH,
            if self.is_in_trash() {
                "Kalıcı Olarak Sil"
            } else {
                "Çöp Kutusuna Gönder"
            },
            "Del",
            true,
            Message::TrashSelected,
            palette,
        ));

        let menu_card = container(items)
            .width(Length::Fixed(268.0))
            .padding(Spacing::SM)
            .style(move |_| corten_theme::iced_styles::acrylic_glass_style(palette, Radius::MD));

        menu_card.into()
    }

    fn view_terminal_drawer(&self, palette: Palette) -> Element<'_, Message> {
        let title_row = row![
            text(Icon::TERMINAL)
                .font(LUCIDE_FONT)
                .size(13.0)
                .color(palette.primary.to_iced()),
            text(format!("Hızlı Komut & Terminal — {}", self.current_dir.display()))
                .size(11.0)
                .color(palette.text_primary.to_iced()),
            horizontal_space(),
            button(text("✕").size(10.0).color(palette.text_muted.to_iced()))
                .padding([1, 4])
                .style(|_, _| button::Style::default())
                .on_press(Message::ToggleTerminalDrawer),
        ]
        .spacing(6)
        .align_y(Alignment::Center);

        let mut log_col = column![].spacing(2);
        if self.terminal_log.is_empty() {
            log_col = log_col.push(
                text("Komut çalıştırmak için aşağıya yazın (örn: git status, cargo test, ls -la, mkdir test)...")
                    .size(11.0)
                    .color(palette.text_muted.to_iced()),
            );
        } else {
            for line in self.terminal_log.iter().rev().take(6).rev() {
                log_col = log_col.push(
                    text(line)
                        .size(11.0)
                        .color(if line.starts_with('$') {
                            palette.primary.to_iced()
                        } else if line.starts_with("err:") {
                            palette.danger.to_iced()
                        } else {
                            palette.text_primary.to_iced()
                        }),
                );
            }
        }

        let input_row = row![
            text("$")
                .size(12.0)
                .color(palette.primary.to_iced()),
            text_input("Komut girin...", &self.terminal_input)
                .size(12.0)
                .on_input(Message::TerminalInputChanged)
                .on_submit(Message::ExecuteTerminalCommand),
            button(
                text("Çalıştır")
                    .size(11.0)
                    .color(palette.background.to_iced())
            )
            .padding([3, 10])
            .style(move |_, _| button::Style {
                background: Some(iced::Background::Color(palette.primary.to_iced())),
                border: Border {
                    radius: border::Radius::from(6.0),
                    ..Default::default()
                },
                ..Default::default()
            })
            .on_press(Message::ExecuteTerminalCommand),
        ]
        .spacing(6)
        .align_y(Alignment::Center);

        container(
            column![title_row, log_col, input_row].spacing(6)
        )
        .padding([8, 12])
        .width(Length::Fill)
        .style(move |_| container::Style {
            background: Some(iced::Background::Color(iced::Color::from_rgba(0.06, 0.07, 0.09, 0.95))),
            border: Border {
                color: iced::Color::from_rgba(palette.primary.r, palette.primary.g, palette.primary.b, 0.35),
                width: 1.0,
                radius: border::Radius::from(8.0),
            },
            ..Default::default()
        })
        .into()
    }

    fn view_tab_bar(&self, palette: Palette) -> Element<'_, Message> {
        let mut tab_row = row![].spacing(4).align_y(Alignment::Center);

        for (idx, tab) in self.tabs.iter().enumerate() {
            let is_active = idx == self.active_tab;
            let folder_label = tab
                .dir
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "/".to_string());

            let mut item_row = row![
                text(Icon::FOLDER)
                    .font(LUCIDE_FONT)
                    .size(12.5)
                    .color(if is_active {
                        palette.primary.to_iced()
                    } else {
                        palette.text_muted.to_iced()
                    }),
                text(folder_label)
                    .size(12.0)
                    .color(if is_active {
                        palette.text_primary.to_iced()
                    } else {
                        palette.text_muted.to_iced()
                    }),
            ]
            .spacing(5)
            .align_y(Alignment::Center);

            if is_active {
                let dup_btn = button(
                    text("⧉")
                        .size(10.5)
                        .color(palette.text_muted.to_iced()),
                )
                .padding([0, 3])
                .on_press(Message::DuplicateTab(idx))
                .style(move |_, _| button::Style {
                    background: None,
                    text_color: palette.text_muted.to_iced(),
                    ..Default::default()
                });
                item_row = item_row.push(dup_btn);
            }

            if self.tabs.len() > 1 {
                let close_btn = button(
                    text("×")
                        .size(12.0)
                        .color(palette.text_muted.to_iced()),
                )
                .padding([0, 3])
                .on_press(Message::CloseTab(idx))
                .style(move |_, _| button::Style {
                    background: None,
                    text_color: palette.text_muted.to_iced(),
                    ..Default::default()
                });
                item_row = item_row.push(close_btn);
            }

            let tab_btn = button(item_row)
                .padding([4, 10])
                .on_press(Message::SwitchTab(idx))
                .style(move |_, status| {
                    if is_active {
                        let mut s = primary_button_style(palette, status);
                        s.background = Some(palette.surface_hover.to_iced().into());
                        s.text_color = palette.text_primary.to_iced();
                        s.border = Border {
                            color: palette.primary.to_iced(),
                            width: 1.5,
                            radius: border::Radius::from(Radius::SM),
                        };
                        s
                    } else {
                        secondary_button_style(palette, status)
                    }
                });

            tab_row = tab_row.push(tab_btn);
        }

        tab_row = tab_row.push(
            button(
                text("+")
                    .size(13.0)
                    .color(palette.primary.to_iced()),
            )
            .padding([3, 8])
            .on_press(Message::NewTab)
            .style(move |_, status| secondary_button_style(palette, status)),
        );

        let has_selection = self.selected_entry().is_some();
        let has_clipboard = self.clipboard.is_some();
        let is_archive = self.selected_entry().map(|e| e.is_archive()).unwrap_or(false);
        let in_trash = self.is_in_trash();
        let current_bookmarked = self
            .config
            .bookmarks
            .contains(&self.current_dir.to_string_lossy().to_string());

        let mut quick_actions = row![
            Self::small_button("+ Klasör", "", Message::PromptNewFolder, palette),
            Self::small_button("+ Dosya", "", Message::PromptNewFile, palette),
            Self::small_button(
                if current_bookmarked { "★ Yer İmi" } else { "☆ Yer İmi" },
                "",
                Message::ToggleBookmarkCurrent,
                palette,
            ),
        ]
        .spacing(4)
        .align_y(Alignment::Center);

        if has_clipboard {
            quick_actions = quick_actions.push(Self::accent_button(
                "Yapıştır",
                Message::PasteClipboard,
                palette,
            ));
        }

        if has_selection {
            quick_actions = quick_actions
                .push(Self::small_button("Adlandır", "", Message::PromptRename, palette))
                .push(Self::small_button(
                    if in_trash { "Kalıcı Sil" } else { "Çöp" },
                    "",
                    Message::TrashSelected,
                    palette,
                ));
        }

        if is_archive {
            quick_actions = quick_actions.push(Self::accent_button(
                "Arşivi Çıkar",
                Message::ExtractSelectedArchive,
                palette,
            ));
        }

        if in_trash {
            if has_selection {
                quick_actions = quick_actions.push(Self::accent_button(
                    "Geri Yükle",
                    Message::RestoreSelectedFromTrash,
                    palette,
                ));
            }
            quick_actions = quick_actions.push(Self::small_button(
                "Boşalt",
                "",
                Message::EmptyTrash,
                palette,
            ));
        }

        quick_actions = quick_actions.push(Self::small_button(
            ">_ Terminal",
            "",
            Message::OpenTerminalHere,
            palette,
        ));

        container(
            row![
                scrollable(tab_row)
                    .direction(scrollable::Direction::Horizontal(
                        scrollable::Scrollbar::new().width(0).scroller_width(0),
                    ))
                    .width(Length::Fill),
                quick_actions,
            ]
            .spacing(Spacing::SM)
            .align_y(Alignment::Center),
        )
        .padding([3, 6])
        .width(Length::Fill)
        .into()
    }

    fn view_top_bar(&self, palette: Palette) -> Element<'_, Message> {
        let nav_buttons = row![
            Self::small_button("←", "Geri", Message::NavigateBack, palette),
            Self::small_button("→", "İleri", Message::NavigateForward, palette),
            Self::small_button("↑", "Üst", Message::NavigateUp, palette),
            Self::small_button("↻", "Yenile", Message::Refresh, palette),
        ]
        .spacing(Spacing::XXS);

        let path_widget: Element<'_, Message> = if self.editing_address {
            row![
                text_input("Dizin yolunu girin...", &self.address_input)
                    .on_input(Message::AddressInputChanged)
                    .on_submit(Message::AddressSubmitted)
                    .padding([5, 10])
                    .size(Typography::BODY_SM)
                    .width(Length::Fill),
                Self::small_button("Git", "", Message::AddressSubmitted, palette),
                Self::small_button("İptal", "", Message::ToggleAddressEdit, palette),
            ]
            .spacing(Spacing::XS)
            .width(Length::Fill)
            .align_y(Alignment::Center)
            .into()
        } else {
            let mut crumb_row = row![].spacing(3).align_y(Alignment::Center);
            let crumbs = breadcrumbs(&self.current_dir);
            for (idx, (label, path)) in crumbs.into_iter().enumerate() {
                if idx > 0 {
                    crumb_row = crumb_row.push(
                        text(Icon::CHEVRON_RIGHT)
                            .font(LUCIDE_FONT)
                            .size(10)
                            .color(palette.text_muted.to_iced()),
                    );
                }
                let is_last = path == self.current_dir;
                let is_dropdown_open = self.breadcrumb_dropdown.as_ref() == Some(&path);
                let icon_ch = if idx == 0 {
                    Icon::USER
                } else {
                    Icon::FOLDER
                };
                let path_for_nav = path.clone();
                let path_for_drop = path.clone();

                let seg_btn = button(
                    row![
                        text(icon_ch)
                            .font(LUCIDE_FONT)
                            .size(11.5)
                            .color(if is_last {
                                palette.primary.to_iced()
                            } else {
                                palette.text_muted.to_iced()
                            }),
                        text(label)
                            .size(12.0)
                            .color(if is_last {
                                palette.primary.to_iced()
                            } else {
                                palette.text_primary.to_iced()
                            }),
                    ]
                    .spacing(4)
                    .align_y(Alignment::Center),
                )
                .padding([3, 8])
                .on_press(Message::NavigateTo(path_for_nav))
                .style(move |_, status| {
                    let bg = match status {
                        button::Status::Hovered => palette.surface_hover.to_iced(),
                        _ if is_last => Color::from_rgba(
                            palette.primary.r,
                            palette.primary.g,
                            palette.primary.b,
                            0.16,
                        ),
                        _ => Color::from_rgba(1.0, 1.0, 1.0, 0.04),
                    };
                    button::Style {
                        background: Some(bg.into()),
                        text_color: palette.text_primary.to_iced(),
                        border: Border {
                            color: if is_last {
                                Color::from_rgba(
                                    palette.primary.r,
                                    palette.primary.g,
                                    palette.primary.b,
                                    0.45,
                                )
                            } else {
                                Color::from_rgba(1.0, 1.0, 1.0, 0.07)
                            },
                            width: 1.0,
                            radius: border::Radius::from(Radius::FULL),
                        },
                        shadow: Shadow::default(),
                    }
                });

                crumb_row = crumb_row.push(seg_btn);

                // Interactive sibling dropdown trigger chevron
                if idx > 0 {
                    let drop_btn = button(
                        text(if is_dropdown_open { "▴" } else { "▾" })
                            .size(10.0)
                            .color(if is_dropdown_open {
                                palette.primary.to_iced()
                            } else {
                                palette.text_muted.to_iced()
                            }),
                    )
                    .padding([2, 4])
                    .on_press(Message::ToggleBreadcrumbDropdown(path_for_drop))
                    .style(move |_, status| {
                        let bg = match status {
                            button::Status::Hovered => palette.surface_hover.to_iced(),
                            _ if is_dropdown_open => Color::from_rgba(
                                palette.primary.r,
                                palette.primary.g,
                                palette.primary.b,
                                0.22,
                            ),
                            _ => Color::TRANSPARENT,
                        };
                        button::Style {
                            background: Some(bg.into()),
                            text_color: palette.text_muted.to_iced(),
                            border: Border {
                                radius: border::Radius::from(Radius::SM),
                                ..Default::default()
                            },
                            shadow: Shadow::default(),
                        }
                    });
                    crumb_row = crumb_row.push(drop_btn);
                }
            }

            container(
                row![
                    scrollable(crumb_row)
                        .direction(scrollable::Direction::Horizontal(
                            scrollable::Scrollbar::new().width(0).scroller_width(0),
                        ))
                        .width(Length::Fill),
                    button(
                        text("✎ Yol")
                            .size(11.0)
                            .color(palette.text_muted.to_iced()),
                    )
                    .padding([2, 7])
                    .on_press(Message::ToggleAddressEdit)
                    .style(move |_, _| secondary_button_style(palette, button::Status::Active)),
                ]
                .align_y(Alignment::Center)
                .spacing(Spacing::XS),
            )
            .padding([3, 8])
            .width(Length::Fill)
            .style(move |_| input_style(palette))
            .into()
        };

        let match_count = if !self.search_query.trim().is_empty() {
            container(
                text(format!("{} öğe", self.entries.len()))
                    .size(10.0)
                    .color(palette.primary.to_iced()),
            )
            .padding([2, 5])
            .style(move |_| container::Style {
                background: Some(
                    Color::from_rgba(
                        palette.primary.r,
                        palette.primary.g,
                        palette.primary.b,
                        0.15,
                    )
                    .into(),
                ),
                border: Border {
                    radius: border::Radius::from(Radius::SM),
                    ..Default::default()
                },
                ..Default::default()
            })
        } else {
            container(row![])
        };

        let search_box = container(
            row![
                text(Icon::SEARCH)
                    .font(LUCIDE_FONT)
                    .size(13)
                    .color(palette.text_muted.to_iced()),
                text_input("Filtrele (Ctrl+F)...", &self.search_query)
                    .on_input(Message::SearchChanged)
                    .padding([3, 6])
                    .size(12.0)
                    .width(Length::Fixed(145.0)),
                match_count,
            ]
            .spacing(Spacing::XS)
            .align_y(Alignment::Center),
        )
        .padding([2, 8])
        .style(move |_| input_style(palette));

        let view_toggles = row![
            Self::toggle_pill(
                "Liste",
                self.config.view_mode == ViewMode::List,
                Message::SetViewMode(ViewMode::List),
                palette,
            ),
            Self::toggle_pill(
                "Izgara",
                self.config.view_mode == ViewMode::Grid,
                Message::SetViewMode(ViewMode::Grid),
                palette,
            ),
            Self::toggle_pill(
                "Sütun",
                self.config.view_mode == ViewMode::Columns,
                Message::SetViewMode(ViewMode::Columns),
                palette,
            ),
            Self::toggle_pill(
                ".gizli",
                self.config.show_hidden,
                Message::ToggleHidden,
                palette,
            ),
            Self::toggle_pill(
                "İkili",
                self.split_view,
                Message::ToggleSplitView,
                palette,
            ),
            Self::toggle_pill(
                "Denetçi",
                self.config.show_inspector,
                Message::ToggleInspector,
                palette,
            ),
        ]
        .spacing(3);

        container(
            row![nav_buttons, path_widget, search_box, view_toggles]
                .spacing(Spacing::SM)
                .align_y(Alignment::Center),
        )
        .padding([6, 10])
        .width(Length::Fill)
        .style(move |_| container::Style {
            background: Some(iced::Background::Color(Color::from_rgba(0.08, 0.09, 0.12, 0.95))),
            border: Border {
                color: Color::from_rgba(1.0, 1.0, 1.0, 0.07),
                width: 1.0,
                radius: border::Radius::from(Radius::MD),
            },
            ..Default::default()
        })
        .into()
    }

    fn view_breadcrumb_dropdown(
        &self,
        target_dir: &std::path::Path,
        palette: Palette,
    ) -> Element<'_, Message> {
        let siblings = sibling_directories(target_dir, self.config.show_hidden);
        let parent_name = target_dir
            .parent()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "/".to_string());

        let mut pills = row![
            text(format!("{} içindeki klasörler:", parent_name))
                .size(11.0)
                .color(palette.text_muted.to_iced()),
        ]
        .spacing(6)
        .align_y(Alignment::Center);

        for (name, path) in siblings {
            let is_current = path == self.current_dir || self.current_dir.starts_with(&path);
            let pill = button(
                row![
                    text(Icon::FOLDER)
                        .font(LUCIDE_FONT)
                        .size(11.5)
                        .color(if is_current {
                            palette.primary.to_iced()
                        } else {
                            palette.text_muted.to_iced()
                        }),
                    text(name).size(11.5).color(if is_current {
                        palette.primary.to_iced()
                    } else {
                        palette.text_primary.to_iced()
                    }),
                ]
                .spacing(4)
                .align_y(Alignment::Center),
            )
            .padding([3, 9])
            .on_press(Message::NavigateTo(path))
            .style(move |_, status| {
                if is_current {
                    primary_button_style(palette, status)
                } else {
                    secondary_button_style(palette, status)
                }
            });
            pills = pills.push(pill);
        }

        container(
            row![
                scrollable(pills)
                    .direction(scrollable::Direction::Horizontal(
                        scrollable::Scrollbar::new().width(0).scroller_width(0),
                    ))
                    .width(Length::Fill),
                button(text("✕").size(11.0).color(palette.text_muted.to_iced()))
                    .padding([2, 6])
                    .on_press(Message::CloseBreadcrumbDropdown)
                    .style(|_, _| button::Style::default()),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        )
        .padding([5, 12])
        .width(Length::Fill)
        .style(move |_| container::Style {
            background: Some(iced::Background::Color(Color::from_rgba(0.09, 0.10, 0.14, 0.96))),
            border: Border {
                color: Color::from_rgba(
                    palette.primary.r,
                    palette.primary.g,
                    palette.primary.b,
                    0.45,
                ),
                width: 1.0,
                radius: border::Radius::from(Radius::MD),
            },
            ..Default::default()
        })
        .into()
    }

    fn view_inline_modal<'a>(
        &'a self,
        modal: &'a InlineModal,
        palette: Palette,
    ) -> Element<'a, Message> {
        let content: Element<'a, Message> = match modal {
            InlineModal::NewFolder(val) => row![
                text("Yeni Klasör Adı:")
                    .size(Typography::BODY_SM)
                    .color(palette.primary.to_iced()),
                text_input("Klasör adı...", val)
                    .on_input(Message::ModalInputChanged)
                    .on_submit(Message::ConfirmModal)
                    .padding([5, 10])
                    .size(Typography::BODY_SM)
                    .width(Length::Fixed(280.0)),
                Self::accent_button("Oluştur", Message::ConfirmModal, palette),
                Self::small_button("İptal", "", Message::CancelModal, palette),
            ]
            .spacing(Spacing::SM)
            .align_y(Alignment::Center)
            .into(),
            InlineModal::NewFile(val) => row![
                text("Yeni Dosya Adı:")
                    .size(Typography::BODY_SM)
                    .color(palette.primary.to_iced()),
                text_input("Dosya adı...", val)
                    .on_input(Message::ModalInputChanged)
                    .on_submit(Message::ConfirmModal)
                    .padding([5, 10])
                    .size(Typography::BODY_SM)
                    .width(Length::Fixed(280.0)),
                Self::accent_button("Oluştur", Message::ConfirmModal, palette),
                Self::small_button("İptal", "", Message::CancelModal, palette),
            ]
            .spacing(Spacing::SM)
            .align_y(Alignment::Center)
            .into(),
            InlineModal::Rename(_, val) => row![
                text("Yeni İsim:")
                    .size(Typography::BODY_SM)
                    .color(palette.primary.to_iced()),
                text_input("Yeni ad...", val)
                    .on_input(Message::ModalInputChanged)
                    .on_submit(Message::ConfirmModal)
                    .padding([5, 10])
                    .size(Typography::BODY_SM)
                    .width(Length::Fixed(280.0)),
                Self::accent_button("Kaydet", Message::ConfirmModal, palette),
                Self::small_button("İptal", "", Message::CancelModal, palette),
            ]
            .spacing(Spacing::SM)
            .align_y(Alignment::Center)
            .into(),
            InlineModal::ConfirmPermanentDelete(path) => row![
                text(format!(
                    "Kalıcı olarak silinsin mi? ({})",
                    path.file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default()
                ))
                .size(Typography::BODY_SM)
                .color(palette.danger.to_iced()),
                horizontal_space(),
                Self::accent_button("Evet, Kalıcı Sil", Message::ConfirmModal, palette),
                Self::small_button("Vazgeç", "", Message::CancelModal, palette),
            ]
            .spacing(Spacing::SM)
            .align_y(Alignment::Center)
            .into(),
            InlineModal::SecurityGatekeeper { path, sha256 } => {
                let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                let short_hash = if sha256.len() > 16 {
                    format!("{}...{}", &sha256[..8], &sha256[sha256.len().saturating_sub(8)..])
                } else {
                    sha256.clone()
                };
                let path_clone = path.clone();
                row![
                    text(Icon::SHIELD)
                        .font(LUCIDE_FONT)
                        .size(14)
                        .color(palette.warning.to_iced()),
                    text("Corten Gatekeeper:")
                        .size(Typography::BODY_SM)
                        .color(palette.warning.to_iced()),
                    text(format!("\"{}\" (SHA256: {}) dış kaynaktan indirildi veya çalıştırılabilir betik.", name, short_hash))
                        .size(Typography::BODY_SM)
                        .color(palette.text_primary.to_iced()),
                    horizontal_space(),
                    Self::small_button("AI Analizi", "", Message::AnalyzeWithAi(path_clone), palette),
                    Self::accent_button("Yine de Çalıştır", Message::ConfirmModal, palette),
                    Self::small_button("İptal", "", Message::CancelModal, palette),
                ]
                .spacing(Spacing::SM)
                .align_y(Alignment::Center)
                .into()
            }
        };

        container(content)
            .padding([Spacing::XS, Spacing::MD])
            .width(Length::Fill)
            .style(move |_| container::Style {
                background: Some(palette.surface_hover.to_iced().into()),
                border: Border {
                    color: palette.primary.to_iced(),
                    width: 1.5,
                    radius: border::Radius::from(Radius::MD),
                },
                ..Default::default()
            })
            .into()
    }

    fn view_sidebar(&self, palette: Palette) -> Element<'_, Message> {
        let mut col = column![
            text("KONUMLAR")
                .size(10.5)
                .color(palette.text_muted.to_iced()),
        ]
        .spacing(3);

        for place in &self.places {
            let is_active = self.current_dir == place.path;
            let icon_ch = if place.is_trash {
                Icon::TRASH
            } else if place.is_mount {
                Icon::CPU
            } else if place.label.starts_with("Ev") {
                Icon::USER
            } else if place.label.starts_with("İndir") {
                Icon::DOWNLOAD
            } else if place.label.starts_with("Resim") {
                Icon::MONITOR
            } else if place.label.starts_with("Proje") {
                Icon::CODE
            } else {
                Icon::FOLDER
            };

            let badge_el: Element<'_, Message> = if let Some(count) = place.item_count {
                container(
                    text(format!("{count}"))
                        .size(9.5)
                        .color(if is_active {
                            Color::WHITE
                        } else {
                            palette.text_muted.to_iced()
                        }),
                )
                .padding([1, 6])
                .style(move |_| container::Style {
                    background: Some(iced::Background::Color(if is_active {
                        Color::from_rgba(1.0, 1.0, 1.0, 0.22)
                    } else {
                        Color::from_rgba(1.0, 1.0, 1.0, 0.06)
                    })),
                    border: Border {
                        radius: border::Radius::from(Radius::FULL),
                        ..Default::default()
                    },
                    ..Default::default()
                })
                .into()
            } else {
                horizontal_space().width(Length::Shrink).into()
            };

            let item = button(
                row![
                    text(icon_ch)
                        .font(LUCIDE_FONT)
                        .size(14.5)
                        .color(if is_active {
                            Color::WHITE
                        } else if place.is_trash {
                            palette.danger.to_iced()
                        } else {
                            palette.primary.to_iced()
                        }),
                    text(place.label.clone())
                        .size(12.0)
                        .color(if is_active {
                            Color::WHITE
                        } else {
                            palette.text_primary.to_iced()
                        })
                        .width(Length::Fill),
                    badge_el,
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            )
            .width(Length::Fill)
            .padding([6, 10])
            .on_press(Message::NavigateTo(place.path.clone()))
            .style(move |_, status| {
                if is_active {
                    primary_button_style(palette, status)
                } else {
                    secondary_button_style(palette, status)
                }
            });

            col = col.push(item);
        }

        if !self.config.bookmarks.is_empty() {
            col = col
                .push(vertical_space().height(Spacing::XS))
                .push(
                    text("YER İMLERİ")
                        .size(10.5)
                        .color(palette.text_muted.to_iced()),
                );

            for bm in &self.config.bookmarks {
                let path = PathBuf::from(bm);
                let label = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| bm.clone());
                let is_active = self.current_dir == path;
                let btn = button(
                    row![
                        text("★")
                            .size(12.0)
                            .color(if is_active {
                                Color::WHITE
                            } else {
                                palette.warning.to_iced()
                            }),
                        text(label).size(12.0),
                    ]
                    .spacing(8)
                    .align_y(Alignment::Center),
                )
                .width(Length::Fill)
                .padding([6, 10])
                .on_press(Message::NavigateTo(path))
                .style(move |_, status| {
                    if is_active {
                        primary_button_style(palette, status)
                    } else {
                        secondary_button_style(palette, status)
                    }
                });
                col = col.push(btn);
            }
        }

        if !self.devices.is_empty() {
            col = col
                .push(vertical_space().height(Spacing::XS))
                .push(
                    text("DEPOLAMA & AYGITLAR")
                        .size(10.5)
                        .color(palette.text_muted.to_iced()),
                );

            for dev in &self.devices {
                let is_active = if let Some(ref mp) = dev.mount_point {
                    self.current_dir.starts_with(mp)
                } else {
                    false
                };

                let dev_label = if dev.size > 0 {
                    format!("{} ({})", dev.label, format_size(dev.size))
                } else {
                    dev.label.clone()
                };

                let action_btn = if let Some(ref mp) = dev.mount_point {
                    let mp_clone = mp.clone();
                    let obj_path = dev.object_path.clone();
                    row![
                        button(
                            row![
                                text(Icon::CPU)
                                    .font(LUCIDE_FONT)
                                    .size(14)
                                    .color(if is_active {
                                        Color::WHITE
                                    } else {
                                        palette.success.to_iced()
                                    }),
                                text(dev_label)
                                    .size(11.5)
                                    .color(if is_active {
                                        Color::WHITE
                                    } else {
                                        palette.text_primary.to_iced()
                                    }),
                            ]
                            .spacing(Spacing::SM)
                            .align_y(Alignment::Center),
                        )
                        .width(Length::Fill)
                        .padding([6, 8])
                        .on_press(Message::NavigateTo(mp_clone))
                        .style(move |_, status| {
                            if is_active {
                                primary_button_style(palette, status)
                            } else {
                                secondary_button_style(palette, status)
                            }
                        }),
                        button(
                            text("⏏")
                                .size(13)
                                .color(palette.text_muted.to_iced()),
                        )
                        .padding([4, 6])
                        .on_press(Message::UnmountDevice(obj_path))
                        .style(move |_, status| secondary_button_style(palette, status)),
                    ]
                    .spacing(Spacing::XXS)
                    .align_y(Alignment::Center)
                } else {
                    let obj_path = dev.object_path.clone();
                    row![
                        text(Icon::CPU)
                            .font(LUCIDE_FONT)
                            .size(14)
                            .color(palette.text_muted.to_iced()),
                        text(dev_label)
                            .size(11.5)
                            .color(palette.text_muted.to_iced())
                            .width(Length::Fill),
                        button(
                            text("Bağla")
                                .size(Typography::CAPTION)
                                .color(palette.primary.to_iced()),
                        )
                        .padding([4, 6])
                        .on_press(Message::MountDevice(obj_path))
                        .style(move |_, status| secondary_button_style(palette, status)),
                    ]
                    .spacing(Spacing::XXS)
                    .align_y(Alignment::Center)
                };

                col = col.push(action_btn);
            }
        }

        // Disk capacity indicator card at the bottom of the sidebar
        let disk_card: Element<'_, Message> = if let Some((used, total, ratio)) = self.disk_usage {
            let free = total.saturating_sub(used);
            let pct = (ratio * 100.0).round() as u32;
            let fill_w = (168.0 * ratio).clamp(6.0, 168.0);
            let bar_color = if ratio > 0.88 {
                palette.danger.to_iced()
            } else if ratio > 0.72 {
                palette.warning.to_iced()
            } else {
                palette.primary.to_iced()
            };

            container(
                column![
                    row![
                        text(Icon::CPU)
                            .font(LUCIDE_FONT)
                            .size(12.0)
                            .color(bar_color),
                        text("Sistem Diski")
                            .size(11.0)
                            .color(palette.text_primary.to_iced())
                            .width(Length::Fill),
                        text(format!("%{pct}"))
                            .size(10.5)
                            .font(iced::Font::MONOSPACE)
                            .color(bar_color),
                    ]
                    .spacing(6)
                    .align_y(Alignment::Center),
                    container(
                        container(row![])
                            .width(Length::Fixed(fill_w))
                            .height(Length::Fixed(4.0))
                            .style(move |_| container::Style {
                                background: Some(iced::Background::Color(bar_color)),
                                border: Border {
                                    radius: border::Radius::from(Radius::FULL),
                                    ..Default::default()
                                },
                                ..Default::default()
                            }),
                    )
                    .width(Length::Fixed(168.0))
                    .height(Length::Fixed(4.0))
                    .style(|_| container::Style {
                        background: Some(iced::Background::Color(Color::from_rgba(
                            1.0, 1.0, 1.0, 0.08,
                        ))),
                        border: Border {
                            radius: border::Radius::from(Radius::FULL),
                            ..Default::default()
                        },
                        ..Default::default()
                    }),
                    text(format!("{} boş / {}", format_size(free), format_size(total)))
                        .size(10.0)
                        .color(palette.text_muted.to_iced()),
                ]
                .spacing(5),
            )
            .padding([8, 10])
            .width(Length::Fill)
            .style(|_| container::Style {
                background: Some(iced::Background::Color(Color::from_rgba(
                    1.0, 1.0, 1.0, 0.03,
                ))),
                border: Border {
                    color: Color::from_rgba(1.0, 1.0, 1.0, 0.07),
                    width: 1.0,
                    radius: border::Radius::from(Radius::SM),
                },
                ..Default::default()
            })
            .into()
        } else {
            vertical_space().height(Length::Shrink).into()
        };

        container(
            column![
                scrollable(col).height(Length::Fill),
                disk_card,
            ]
            .spacing(Spacing::XS)
            .height(Length::Fill),
        )
        .width(Length::Fixed(214.0))
        .height(Length::Fill)
        .padding(Spacing::SM)
        .style(move |_| container::Style {
            background: Some(iced::Background::Color(Color::from_rgba(0.06, 0.07, 0.09, 0.75))),
            border: Border {
                color: Color::from_rgba(1.0, 1.0, 1.0, 0.05),
                width: 1.0,
                radius: border::Radius::from(0.0),
            },
            ..Default::default()
        })
        .into()
    }

    fn view_explorer_pane(&self, palette: Palette) -> Element<'_, Message> {
        if self.config.view_mode == ViewMode::Columns {
            return self.view_miller_columns_pane(palette);
        }

        let header = row![
            Self::sort_header_btn(
                "İsim",
                SortColumn::Name,
                self.config.sort_column,
                self.config.sort_direction,
                Length::FillPortion(5),
                palette,
            ),
            Self::sort_header_btn(
                "Tür",
                SortColumn::Kind,
                self.config.sort_column,
                self.config.sort_direction,
                Length::FillPortion(2),
                palette,
            ),
            Self::sort_header_btn(
                "Boyut",
                SortColumn::Size,
                self.config.sort_column,
                self.config.sort_direction,
                Length::FillPortion(2),
                palette,
            ),
            Self::sort_header_btn(
                "Değiştirilme",
                SortColumn::Modified,
                self.config.sort_column,
                self.config.sort_direction,
                Length::FillPortion(2),
                palette,
            ),
        ]
        .spacing(Spacing::XS)
        .padding([4, 8]);

        let body: Element<'_, Message> = if self.entries.is_empty() {
            container(
                column![
                    text(Icon::FOLDER)
                        .font(LUCIDE_FONT)
                        .size(36)
                        .color(palette.text_muted.to_iced()),
                    text("Bu klasör boş")
                        .size(Typography::BODY)
                        .color(palette.text_muted.to_iced()),
                ]
                .spacing(Spacing::XS)
                .align_x(Alignment::Center),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into()
        } else if self.config.view_mode == ViewMode::List {
            let mut list_col = column![].spacing(3);
            for (idx, entry) in self.entries.iter().enumerate() {
                let is_selected = self.selected_index == Some(idx);
                let icon_ch = Self::entry_icon(entry.kind);
                let accent = Self::entry_accent_color(entry, palette);
                let ext_badge = Self::entry_badge_text(entry);
                let size_text = if entry.is_dir {
                    "--".to_string()
                } else {
                    format_size(entry.size)
                };
                let time_text = format_relative_time(entry.modified_secs);

                let row_btn = button(
                    row![
                        row![
                            text(icon_ch)
                                .font(LUCIDE_FONT)
                                .size(15.5)
                                .color(if is_selected { Color::WHITE } else { accent }),
                            text(entry.name.clone())
                                .size(12.5)
                                .color(if is_selected {
                                    Color::WHITE
                                } else {
                                    palette.text_primary.to_iced()
                                })
                                .width(Length::Fill),
                            container(
                                text(ext_badge)
                                    .size(9.5)
                                    .font(iced::Font::MONOSPACE)
                                    .color(if is_selected {
                                        Color::WHITE
                                    } else {
                                        accent
                                    }),
                            )
                            .padding([1, 6])
                            .style(move |_| container::Style {
                                background: Some(iced::Background::Color(if is_selected {
                                    Color::from_rgba(1.0, 1.0, 1.0, 0.20)
                                } else {
                                    Color::from_rgba(accent.r, accent.g, accent.b, 0.14)
                                })),
                                border: Border {
                                    radius: border::Radius::from(Radius::FULL),
                                    ..Default::default()
                                },
                                ..Default::default()
                            }),
                        ]
                        .spacing(8)
                        .align_y(Alignment::Center)
                        .width(Length::FillPortion(5)),
                        text(entry.kind.label())
                            .size(11.5)
                            .color(if is_selected {
                                Color::WHITE
                            } else {
                                palette.text_muted.to_iced()
                            })
                            .width(Length::FillPortion(2)),
                        text(size_text)
                            .size(11.5)
                            .font(iced::Font::MONOSPACE)
                            .color(if is_selected {
                                Color::WHITE
                            } else {
                                palette.text_muted.to_iced()
                            })
                            .width(Length::FillPortion(2)),
                        text(time_text)
                            .size(11.5)
                            .color(if is_selected {
                                Color::WHITE
                            } else {
                                palette.text_muted.to_iced()
                            })
                            .width(Length::FillPortion(2)),
                    ]
                    .align_y(Alignment::Center),
                )
                .width(Length::Fill)
                .padding([6, 10])
                .on_press(Message::SelectIndex(idx))
                .style(move |_, status| {
                    if is_selected {
                        primary_button_style(palette, status)
                    } else {
                        secondary_button_style(palette, status)
                    }
                });

                let row_area = mouse_area(row_btn).on_right_press(Message::OpenContextMenu(idx));
                list_col = list_col.push(row_area);
            }
            scrollable(list_col).height(Length::Fill).into()
        } else {
            // Upgraded Grid view (3 cards per row with frosted preview stage & extension badge)
            let mut grid_col = column![].spacing(Spacing::SM);
            for chunk in self.entries.chunks(3) {
                let mut r = row![].spacing(Spacing::SM);
                for entry in chunk {
                    let idx = self
                        .entries
                        .iter()
                        .position(|e| e.path == entry.path)
                        .unwrap_or(0);
                    let is_selected = self.selected_index == Some(idx);
                    let icon_ch = Self::entry_icon(entry.kind);
                    let accent = Self::entry_accent_color(entry, palette);
                    let badge_txt = Self::entry_badge_text(entry);
                    let subtitle = if entry.is_dir {
                        format!("Klasör • {}", format_relative_time(entry.modified_secs))
                    } else {
                        format!(
                            "{} • {}",
                            format_size(entry.size),
                            format_relative_time(entry.modified_secs)
                        )
                    };

                    let preview_widget: Element<'_, Message> =
                        if let Some(path) = fs_ops::get_image_preview_path(&entry.path) {
                            iced::widget::image(iced::widget::image::Handle::from_path(path))
                                .width(Length::Fixed(80.0))
                                .height(Length::Fixed(52.0))
                                .content_fit(iced::ContentFit::Cover)
                                .into()
                        } else {
                            container(
                                text(icon_ch).font(LUCIDE_FONT).size(26).color(if is_selected {
                                    Color::WHITE
                                } else {
                                    accent
                                }),
                            )
                            .padding([10, 14])
                            .style(move |_| container::Style {
                                background: Some(iced::Background::Color(if is_selected {
                                    Color::from_rgba(1.0, 1.0, 1.0, 0.16)
                                } else {
                                    Color::from_rgba(accent.r, accent.g, accent.b, 0.14)
                                })),
                                border: Border {
                                    color: Color::from_rgba(accent.r, accent.g, accent.b, 0.28),
                                    width: 1.0,
                                    radius: border::Radius::from(Radius::MD),
                                },
                                ..Default::default()
                            })
                            .into()
                        };

                    let card = button(
                        column![
                            row![
                                preview_widget,
                                horizontal_space(),
                                container(
                                    text(badge_txt)
                                        .size(9.5)
                                        .font(iced::Font::MONOSPACE)
                                        .color(if is_selected {
                                            Color::WHITE
                                        } else {
                                            accent
                                        }),
                                )
                                .padding([2, 6])
                                .style(move |_| container::Style {
                                    background: Some(iced::Background::Color(if is_selected {
                                        Color::from_rgba(1.0, 1.0, 1.0, 0.20)
                                    } else {
                                        Color::from_rgba(accent.r, accent.g, accent.b, 0.14)
                                    })),
                                    border: Border {
                                        radius: border::Radius::from(Radius::FULL),
                                        ..Default::default()
                                    },
                                    ..Default::default()
                                }),
                            ]
                            .align_y(Alignment::Start),
                            text(entry.name.clone())
                                .size(12.5)
                                .color(if is_selected {
                                    Color::WHITE
                                } else {
                                    palette.text_primary.to_iced()
                                }),
                            text(subtitle)
                                .size(10.5)
                                .color(if is_selected {
                                    Color::WHITE
                                } else {
                                    palette.text_muted.to_iced()
                                }),
                        ]
                        .spacing(6),
                    )
                    .width(Length::FillPortion(1))
                    .padding(Spacing::SM)
                    .on_press(Message::SelectIndex(idx))
                    .style(move |_, status| {
                        if is_selected {
                            primary_button_style(palette, status)
                        } else {
                            secondary_button_style(palette, status)
                        }
                    });
                    let card_area = mouse_area(card).on_right_press(Message::OpenContextMenu(idx));
                    r = r.push(card_area);
                }
                grid_col = grid_col.push(r);
            }
            scrollable(grid_col).height(Length::Fill).into()
        };

        container(column![header, body].spacing(Spacing::XS).height(Length::Fill))
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(Spacing::SM)
            .style(move |_| container::Style {
                background: Some(iced::Background::Color(Color::from_rgba(0.08, 0.09, 0.12, 0.50))),
                border: Border {
                    color: Color::from_rgba(1.0, 1.0, 1.0, 0.04),
                    width: 1.0,
                    radius: border::Radius::from(0.0),
                },
                ..Default::default()
            })
            .into()
    }

    fn view_miller_columns_pane<'a>(&'a self, palette: Palette) -> Element<'a, Message> {
        // Column 1: Parent directory sibling folders
        let siblings = sibling_directories(&self.current_dir, self.config.show_hidden);
        let parent_title = self
            .current_dir
            .parent()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "/".to_string());

        let mut col1_list = column![
            text(format!("ÜST DİZİN ({})", parent_title))
                .size(10.5)
                .color(palette.text_muted.to_iced()),
        ]
        .spacing(3);

        for (name, path) in siblings {
            let is_current = path == self.current_dir;
            let btn = button(
                row![
                    text(Icon::FOLDER)
                        .font(LUCIDE_FONT)
                        .size(13.5)
                        .color(if is_current {
                            Color::WHITE
                        } else {
                            palette.primary.to_iced()
                        }),
                    text(name)
                        .size(12.0)
                        .color(if is_current {
                            Color::WHITE
                        } else {
                            palette.text_primary.to_iced()
                        })
                        .width(Length::Fill),
                    text(Icon::CHEVRON_RIGHT)
                        .font(LUCIDE_FONT)
                        .size(11.0)
                        .color(if is_current {
                            Color::WHITE
                        } else {
                            palette.text_muted.to_iced()
                        }),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
            )
            .width(Length::Fill)
            .padding([5, 8])
            .on_press(Message::NavigateTo(path))
            .style(move |_, status| {
                if is_current {
                    primary_button_style(palette, status)
                } else {
                    secondary_button_style(palette, status)
                }
            });
            col1_list = col1_list.push(btn);
        }

        // Column 2: Current directory entries
        let current_title = self
            .current_dir
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "/".to_string());

        let mut col2_list = column![
            text(format!(
                "AKTİF KLASÖR: {} ({})",
                current_title,
                self.entries.len()
            ))
            .size(10.5)
            .color(palette.primary.to_iced()),
        ]
        .spacing(3);

        for (idx, entry) in self.entries.iter().enumerate() {
            let is_selected = self.selected_index == Some(idx);
            let icon_ch = Self::entry_icon(entry.kind);
            let accent = Self::entry_accent_color(entry, palette);
            let btn = button(
                row![
                    text(icon_ch)
                        .font(LUCIDE_FONT)
                        .size(14.0)
                        .color(if is_selected { Color::WHITE } else { accent }),
                    text(entry.name.clone())
                        .size(12.0)
                        .color(if is_selected {
                            Color::WHITE
                        } else {
                            palette.text_primary.to_iced()
                        })
                        .width(Length::Fill),
                    if entry.is_dir {
                        text(Icon::CHEVRON_RIGHT)
                            .font(LUCIDE_FONT)
                            .size(11.0)
                            .color(if is_selected {
                                Color::WHITE
                            } else {
                                palette.text_muted.to_iced()
                            })
                    } else {
                        text(format_size(entry.size))
                            .size(10.5)
                            .font(iced::Font::MONOSPACE)
                            .color(if is_selected {
                                Color::WHITE
                            } else {
                                palette.text_muted.to_iced()
                            })
                    },
                ]
                .spacing(6)
                .align_y(Alignment::Center),
            )
            .width(Length::Fill)
            .padding([5, 8])
            .on_press(Message::SelectIndex(idx))
            .style(move |_, status| {
                if is_selected {
                    primary_button_style(palette, status)
                } else {
                    secondary_button_style(palette, status)
                }
            });
            col2_list = col2_list.push(mouse_area(btn).on_right_press(Message::OpenContextMenu(idx)));
        }

        // Column 3: Child directory preview OR file inspector preview
        let col3_widget: Element<'_, Message> = if let Some(selected) = self.selected_entry() {
            if selected.is_dir {
                let children = read_directory(
                    &selected.path,
                    self.config.show_hidden,
                    "",
                    self.config.sort_column,
                    self.config.sort_direction,
                )
                .unwrap_or_default();

                let mut child_col = column![
                    text(format!("ALT KLASÖR: {} ({})", selected.name, children.len()))
                        .size(10.5)
                        .color(palette.text_muted.to_iced()),
                ]
                .spacing(3);

                if children.is_empty() {
                    child_col = child_col.push(
                        text("(Boş klasör)")
                            .size(11.5)
                            .color(palette.text_muted.to_iced()),
                    );
                } else {
                    for child in children.into_iter().take(40) {
                        let c_icon = Self::entry_icon(child.kind);
                        let c_accent = Self::entry_accent_color(&child, palette);
                        let target_path = if child.is_dir {
                            child.path.clone()
                        } else {
                            selected.path.clone()
                        };
                        let c_btn = button(
                            row![
                                text(c_icon)
                                    .font(LUCIDE_FONT)
                                    .size(13.5)
                                    .color(c_accent),
                                text(child.name)
                                    .size(11.5)
                                    .color(palette.text_primary.to_iced())
                                    .width(Length::Fill),
                                text(if child.is_dir {
                                    "›".to_string()
                                } else {
                                    format_size(child.size)
                                })
                                .size(10.5)
                                .color(palette.text_muted.to_iced()),
                            ]
                            .spacing(6)
                            .align_y(Alignment::Center),
                        )
                        .width(Length::Fill)
                        .padding([4, 8])
                        .on_press(Message::NavigateTo(target_path))
                        .style(move |_, status| secondary_button_style(palette, status));
                        child_col = child_col.push(c_btn);
                    }
                }
                scrollable(child_col).height(Length::Fill).into()
            } else {
                self.view_inspector_pane(palette)
            }
        } else {
            container(
                text("Önizleme için bir öğe seçin")
                    .size(12.0)
                    .color(palette.text_muted.to_iced()),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into()
        };

        let col_box = |inner: Element<'a, Message>, portion: u16| -> Element<'a, Message> {
            container(inner)
                .width(Length::FillPortion(portion))
                .height(Length::Fill)
                .padding(Spacing::SM)
                .style(move |_| container::Style {
                    background: Some(iced::Background::Color(Color::from_rgba(
                        0.07, 0.08, 0.11, 0.65,
                    ))),
                    border: Border {
                        color: Color::from_rgba(1.0, 1.0, 1.0, 0.06),
                        width: 1.0,
                        radius: border::Radius::from(Radius::SM),
                    },
                    ..Default::default()
                })
                .into()
        };

        container(
            row![
                col_box(scrollable(col1_list).height(Length::Fill).into(), 2),
                col_box(scrollable(col2_list).height(Length::Fill).into(), 3),
                col_box(col3_widget, 3),
            ]
            .spacing(Spacing::XS)
            .height(Length::Fill),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(Spacing::XS)
        .into()
    }

    fn view_right_explorer_pane(&self, palette: Palette) -> Element<'_, Message> {
        let entries_count = self.right_pane.entries.len();
        let header = row![
            text(format!(
                "Sağ Panel: {} ({} öğe)",
                self.right_pane.dir.display(),
                entries_count
            ))
            .size(Typography::BODY_SM)
            .color(palette.primary.to_iced()),
            horizontal_space(),
            Self::small_button("Taşı (F6)", "", Message::MoveToOtherPane, palette),
            Self::small_button("Kopyala (F5)", "", Message::CopyToOtherPane, palette),
        ]
        .spacing(Spacing::XS)
        .align_y(Alignment::Center);

        let mut rows = column![].spacing(2);
        for (idx, entry) in self.right_pane.entries.iter().enumerate() {
            let is_selected = self.right_pane.selected_index == Some(idx);
            let icon_char = Self::entry_icon(entry.kind);
            let icon_color = Self::entry_accent_color(entry, palette);
            let r = row![
                text(icon_char)
                    .font(LUCIDE_FONT)
                    .size(14)
                    .color(if is_selected {
                        Color::WHITE
                    } else {
                        icon_color
                    }),
                text(entry.name.clone())
                    .size(Typography::BODY_SM)
                    .color(if is_selected {
                        Color::WHITE
                    } else {
                        palette.text_primary.to_iced()
                    })
                    .width(Length::Fill),
                text(format_size(entry.size))
                    .size(Typography::CAPTION)
                    .color(if is_selected {
                        Color::WHITE
                    } else {
                        palette.text_muted.to_iced()
                    })
                    .width(Length::Fixed(80.0)),
            ]
            .spacing(Spacing::SM)
            .align_y(Alignment::Center);

            let row_btn = button(r)
                .width(Length::Fill)
                .padding([4, 8])
                .on_press(Message::FocusPane(ActivePane::Right))
                .style(move |_, status| {
                    if is_selected {
                        primary_button_style(palette, status)
                    } else {
                        secondary_button_style(palette, status)
                    }
                });
            rows = rows.push(row_btn);
        }

        container(column![header, scrollable(rows).height(Length::Fill)].spacing(Spacing::XS).height(Length::Fill))
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(Spacing::SM)
            .style(move |_| card_style(palette))
            .into()
    }

    fn view_inspector_pane(&self, palette: Palette) -> Element<'_, Message> {
        let content: Element<'_, Message> = if let Some(ref info) = self.inspection {
            let selected = self.selected_entry();
            let icon_ch = selected
                .map(|e| Self::entry_icon(e.kind))
                .unwrap_or(Icon::APP_WINDOW);
            let accent = selected
                .map(|e| Self::entry_accent_color(e, palette))
                .unwrap_or_else(|| palette.primary.to_iced());

            let hero_header = container(
                row![
                    container(
                        text(icon_ch)
                            .font(LUCIDE_FONT)
                            .size(22)
                            .color(accent),
                    )
                    .padding([8, 10])
                    .style(move |_| container::Style {
                        background: Some(iced::Background::Color(Color::from_rgba(
                            accent.r, accent.g, accent.b, 0.16,
                        ))),
                        border: Border {
                            color: Color::from_rgba(accent.r, accent.g, accent.b, 0.35),
                            width: 1.0,
                            radius: border::Radius::from(Radius::MD),
                        },
                        ..Default::default()
                    }),
                    column![
                        text(info.name.clone())
                            .size(13.5)
                            .color(palette.text_primary.to_iced()),
                        row![
                            container(
                                text(info.extension_upper.clone())
                                    .size(9.5)
                                    .font(iced::Font::MONOSPACE)
                                    .color(accent),
                            )
                            .padding([1, 6])
                            .style(move |_| container::Style {
                                background: Some(iced::Background::Color(Color::from_rgba(
                                    accent.r, accent.g, accent.b, 0.14,
                                ))),
                                border: Border {
                                    radius: border::Radius::from(Radius::FULL),
                                    ..Default::default()
                                },
                                ..Default::default()
                            }),
                            text(info.kind_label)
                                .size(11.0)
                                .color(palette.text_muted.to_iced()),
                        ]
                        .spacing(6)
                        .align_y(Alignment::Center),
                    ]
                    .spacing(3),
                ]
                .spacing(10)
                .align_y(Alignment::Center),
            )
            .padding([8, 10])
            .width(Length::Fill)
            .style(|_| container::Style {
                background: Some(iced::Background::Color(Color::from_rgba(
                    1.0, 1.0, 1.0, 0.03,
                ))),
                border: Border {
                    color: Color::from_rgba(1.0, 1.0, 1.0, 0.07),
                    width: 1.0,
                    radius: border::Radius::from(Radius::MD),
                },
                ..Default::default()
            });

            let mut col = column![
                text("DENETÇİ (INSPECTOR)")
                    .size(10.5)
                    .color(palette.text_muted.to_iced()),
                hero_header,
                Self::meta_row("Boyut", &info.size_display, palette),
                Self::meta_row("Erişim", info.human_permissions, palette),
                Self::meta_row("Mod", &info.permissions, palette),
                Self::meta_row("Değiştirilme", &info.modified_display, palette),
            ]
            .spacing(5)
            .height(Length::Fill);

            if let Some((w, h)) = info.image_dimensions {
                col = col.push(Self::meta_row(
                    "Çözünürlük",
                    &format!("{w} × {h} px"),
                    palette,
                ));
            }
            if let Some(lines) = info.line_count {
                col = col.push(Self::meta_row("Satır Sayısı", &format!("{lines} satır"), palette));
            }

            // Image thumbnail OR Audio Waveform OR Line-numbered Code Preview
            if let Some(img_path) = selected.and_then(|e| fs_ops::get_image_preview_path(&e.path))
            {
                col = col.push(
                    container(
                        iced::widget::image(iced::widget::image::Handle::from_path(img_path))
                            .width(Length::Fill)
                            .height(Length::Fixed(130.0))
                            .content_fit(iced::ContentFit::Contain),
                    )
                    .padding(6)
                    .width(Length::Fill)
                    .style(move |_| input_style(palette)),
                );
                col = col.push(vertical_space());
            } else if selected.map(|e| e.kind == EntryKind::AudioVideo).unwrap_or(false) {
                // Audio / Media Waveform Visual Card
                let mut wave_row = row![].spacing(3).align_y(Alignment::Center);
                for h in [10.0, 18.0, 26.0, 14.0, 30.0, 22.0, 12.0, 28.0, 20.0, 15.0, 24.0, 12.0, 19.0, 25.0, 14.0, 10.0] {
                    wave_row = wave_row.push(
                        container(row![])
                            .width(Length::Fixed(4.0))
                            .height(Length::Fixed(h))
                            .style(move |_| container::Style {
                                background: Some(iced::Background::Color(accent)),
                                border: Border {
                                    radius: border::Radius::from(Radius::FULL),
                                    ..Default::default()
                                },
                                ..Default::default()
                            }),
                    );
                }
                col = col
                    .push(
                        container(
                            column![
                                text("MEDYA AKUSTİK ÖNİZLEME")
                                    .size(9.5)
                                    .color(palette.text_muted.to_iced()),
                                wave_row,
                            ]
                            .spacing(8)
                            .align_x(Alignment::Center),
                        )
                        .padding(10)
                        .width(Length::Fill)
                        .center_x(Length::Fill)
                        .style(move |_| input_style(palette)),
                    )
                    .push(vertical_space());
            } else if let Some(ref preview) = info.text_preview {
                let mut code_lines = column![].spacing(2);
                for (ln, line_txt) in preview.lines().take(24).enumerate() {
                    code_lines = code_lines.push(
                        row![
                            text(format!("{:>2}", ln + 1))
                                .font(iced::Font::MONOSPACE)
                                .size(10.0)
                                .color(Color::from_rgba(1.0, 1.0, 1.0, 0.28))
                                .width(Length::Fixed(20.0)),
                            text(line_txt.to_string())
                                .font(iced::Font::MONOSPACE)
                                .size(11.0)
                                .color(palette.text_primary.to_iced()),
                        ]
                        .spacing(6),
                    );
                }
                col = col
                    .push(
                        text("KOD & METİN ÖNİZLEME")
                            .size(10.0)
                            .color(palette.text_muted.to_iced()),
                    )
                    .push(
                        container(scrollable(code_lines).height(Length::Fill))
                            .padding(8)
                            .width(Length::Fill)
                            .height(Length::Fill)
                            .style(move |_| input_style(palette)),
                    );
            } else {
                col = col.push(vertical_space());
            }

            let is_script_or_binary = info.full_path.ends_with(".sh")
                || info.full_path.ends_with(".py")
                || info.full_path.ends_with(".bash")
                || info.full_path.ends_with(".AppImage")
                || info.full_path.ends_with(".bin")
                || info.permissions.contains('x');

            if is_script_or_binary {
                col = col.push(Self::small_button(
                    "⚡ Corten AI Güvenlik Analizi",
                    "",
                    Message::AnalyzeWithAi(PathBuf::from(&info.full_path)),
                    palette,
                ));
            }

            col.push(
                row![
                    Self::small_button(
                        "👁 Hızlı Bakış (Space)",
                        "",
                        Message::ToggleQuickLook,
                        palette,
                    ),
                    Self::accent_button("Aç (Enter)", Message::ActivateSelected, palette),
                ]
                .spacing(6),
            )
            .into()
        } else {
            column![
                text("DENETÇİ")
                    .size(Typography::CAPTION)
                    .color(palette.text_muted.to_iced()),
                text("Önizleme ve detayları görmek için bir dosya veya klasör seçin.")
                    .size(Typography::CAPTION)
                    .color(palette.text_muted.to_iced()),
            ]
            .spacing(Spacing::SM)
            .into()
        };

        container(content)
            .width(Length::Fixed(288.0))
            .height(Length::Fill)
            .padding(Spacing::SM)
            .style(move |_| card_style(palette))
            .into()
    }

    fn view_status_bar(&self, palette: Palette) -> Element<'_, Message> {
        let clip_text = match &self.clipboard {
            Some(ClipboardOp::Copy(p)) => format!(
                "Pano (Kopyala): {}",
                p.file_name().unwrap_or_default().to_string_lossy()
            ),
            Some(ClipboardOp::Cut(p)) => format!(
                "Pano (Kes): {}",
                p.file_name().unwrap_or_default().to_string_lossy()
            ),
            None => "Kısayollar: Enter Aç | Space Önizle | F2 Adlandır | F3 İkili | F4 Terminal | Ctrl+1/2/3 Görünüm".into(),
        };

        container(
            row![
                text(self.status_message.clone())
                    .size(Typography::CAPTION)
                    .color(palette.primary.to_iced()),
                horizontal_space(),
                text(clip_text)
                    .size(Typography::CAPTION)
                    .color(palette.text_muted.to_iced()),
            ]
            .align_y(Alignment::Center),
        )
        .padding([Spacing::XS + 2.0, Spacing::MD])
        .width(Length::Fill)
        .style(move |_| container::Style {
            background: Some(iced::Background::Color(Color::from_rgba(0.06, 0.07, 0.09, 0.90))),
            border: Border {
                color: Color::from_rgba(1.0, 1.0, 1.0, 0.05),
                width: 1.0,
                radius: border::Radius::from(Radius::MD),
            },
            ..Default::default()
        })
        .into()
    }

    fn meta_row<'a>(label: &'a str, val: &str, palette: Palette) -> Element<'a, Message> {
        row![
            text(format!("{label}:"))
                .size(Typography::CAPTION)
                .color(palette.text_muted.to_iced())
                .width(Length::Fixed(85.0)),
            text(val.to_string())
                .size(Typography::CAPTION)
                .color(palette.text_primary.to_iced()),
        ]
        .into()
    }

    fn sort_header_btn(
        label: &str,
        col: SortColumn,
        active_col: SortColumn,
        dir: SortDirection,
        width: Length,
        palette: Palette,
    ) -> Element<'_, Message> {
        let arrow = if col == active_col {
            match dir {
                SortDirection::Ascending => " ▲",
                SortDirection::Descending => " ▼",
            }
        } else {
            ""
        };
        button(
            text(format!("{label}{arrow}"))
                .size(Typography::CAPTION)
                .color(if col == active_col {
                    palette.primary.to_iced()
                } else {
                    palette.text_muted.to_iced()
                }),
        )
        .width(width)
        .padding([4, 6])
        .on_press(Message::SortBy(col))
        .style(move |_, _| button::Style {
            background: Some(Color::TRANSPARENT.into()),
            text_color: palette.text_muted.to_iced(),
            border: Border::default(),
            shadow: Shadow::default(),
        })
        .into()
    }

    fn small_button<'a>(
        label: &'a str,
        _tooltip: &'a str,
        msg: Message,
        palette: Palette,
    ) -> Element<'a, Message> {
        button(text(label).size(Typography::CAPTION))
            .padding([5, 10])
            .on_press(msg)
            .style(move |_, status| secondary_button_style(palette, status))
            .into()
    }

    fn accent_button(label: &str, msg: Message, palette: Palette) -> Element<'_, Message> {
        button(text(label).size(Typography::CAPTION))
            .padding([5, 12])
            .on_press(msg)
            .style(move |_, status| primary_button_style(palette, status))
            .into()
    }

    fn toggle_pill(
        label: &str,
        active: bool,
        msg: Message,
        palette: Palette,
    ) -> Element<'_, Message> {
        button(text(label).size(Typography::CAPTION))
            .padding([5, 10])
            .on_press(msg)
            .style(move |_, status| {
                if active {
                    primary_button_style(palette, status)
                } else {
                    secondary_button_style(palette, status)
                }
            })
            .into()
    }

    fn entry_icon(kind: EntryKind) -> char {
        match kind {
            EntryKind::Directory => Icon::FOLDER,
            EntryKind::Code => Icon::CODE,
            EntryKind::Image => Icon::MONITOR,
            EntryKind::AudioVideo => Icon::MUSIC,
            EntryKind::Executable => Icon::TERMINAL,
            EntryKind::Archive => Icon::CLIPBOARD,
            EntryKind::Document | EntryKind::File => Icon::APP_WINDOW,
        }
    }

    fn entry_color(kind: EntryKind, palette: Palette) -> Color {
        match kind {
            EntryKind::Directory => palette.primary.to_iced(),
            EntryKind::Code => palette.success.to_iced(),
            EntryKind::Executable => palette.warning.to_iced(),
            _ => palette.text_muted.to_iced(),
        }
    }

    fn entry_accent_color(entry: &FileEntry, palette: Palette) -> Color {
        if entry.is_dir {
            return palette.primary.to_iced();
        }
        let ext = entry
            .path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        match ext.as_str() {
            "rs" | "toml" => Color::from_rgb(0.95, 0.50, 0.24), // Rust Orange
            "py" | "ts" | "js" | "go" | "c" | "cpp" | "h" => Color::from_rgb(0.32, 0.68, 0.96), // Code Cyan/Blue
            "zip" | "tar" | "gz" | "xz" | "7z" | "zst" => palette.warning.to_iced(), // Archive Amber
            "png" | "jpg" | "jpeg" | "webp" | "svg" | "gif" => Color::from_rgb(0.74, 0.48, 0.96), // Image Amethyst
            "mp3" | "flac" | "wav" | "ogg" | "mp4" | "mkv" => palette.success.to_iced(), // Media Teal
            "pdf" | "md" | "doc" | "docx" => Color::from_rgb(0.94, 0.40, 0.44), // Document Coral
            _ => Self::entry_color(entry.kind, palette),
        }
    }

    fn entry_badge_text(entry: &FileEntry) -> String {
        if entry.is_dir {
            return "DIR".to_string();
        }
        entry
            .path
            .extension()
            .and_then(|e| e.to_str())
            .filter(|s| !s.is_empty() && s.len() <= 5)
            .map(|s| s.to_ascii_uppercase())
            .unwrap_or_else(|| entry.kind.badge().to_string())
    }
}
