mod package;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

use corten_config::{AppearanceConfig, ConfigManager};
use corten_theme::iced_styles::card_style;
use corten_theme::{
    Icon, LUCIDE_FONT, LUCIDE_FONT_BYTES, NOTO_SANS, NOTO_SANS_BYTES, Palette, PatinaVariant,
    Radius, Spacing, ThemeMode, Typography,
};
use iced::widget::{
    button, column, container, horizontal_space, image, row, scrollable, text, text_input,
    vertical_space,
};
use iced::{Alignment, ContentFit, Element, Event, Length, Subscription, Task, event};

use package::{PackageItem, PackageManager, PackageSource, PackageState, ProductCategory};

pub fn main() -> iced::Result {
    tracing_subscriber::fmt::init();

    iced::application("Corten Forge", CortenForgeApp::update, CortenForgeApp::view)
        .window(iced::window::Settings {
            size: iced::Size::new(1180.0, 760.0),
            min_size: Some(iced::Size::new(940.0, 600.0)),
            resizable: true,
            #[cfg(target_os = "linux")]
            platform_specific: iced::window::settings::PlatformSpecific {
                application_id: "corten-forge".into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .subscription(CortenForgeApp::subscription)
        .font(LUCIDE_FONT_BYTES)
        .font(NOTO_SANS_BYTES)
        .default_font(NOTO_SANS)
        .run_with(CortenForgeApp::new)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NavigationTab {
    Discover,
    Categories(ProductCategory),
    Installed,
    Updates,
    Detail(String), // package id
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceFilter {
    All,
    FlatpakOnly,
    ArchOnly,
}

#[derive(Debug, Clone)]
pub enum Message {
    CatalogLoaded(Vec<PackageItem>),
    CategoryCatalogLoaded(Vec<PackageItem>),
    InstalledLoaded(Vec<PackageItem>),
    UpdatesLoaded(Vec<PackageItem>),
    IconsLoaded(Vec<(String, Vec<u8>)>),
    AppDetailsLoaded(Option<package::AppDetails>),
    ScreenshotsLoaded(String, Vec<(Vec<u8>, Option<String>)>),
    SelectScreenshot(usize),
    OpenHomepage(String),
    PermissionsLoaded(String, Option<package::SandboxPermissions>),
    NavigateTo(NavigationTab),
    SetSourceFilter(SourceFilter),
    SearchInputChanged(String),
    PerformSearch,
    SearchResultsLoaded(Vec<PackageItem>),
    OpenPackageDetail(PackageItem),
    InstallRequested(PackageItem),
    UninstallRequested(PackageItem),
    LaunchApp(PackageItem),
    LogLineReceived(String),
    OperationFinished(Result<(), String>),
    RefreshAll,
    EventTriggered(Event),
}

pub struct CortenForgeApp {
    current_tab: NavigationTab,
    source_filter: SourceFilter,
    search_query: String,
    catalog: Vec<PackageItem>,
    installed: Vec<PackageItem>,
    updates: Vec<PackageItem>,
    search_results: Option<Vec<PackageItem>>,
    selected_package: Option<PackageItem>,
    active_logs: Vec<String>,
    is_busy: bool,
    busy_message: Option<String>,
    log_rx: Option<Arc<Mutex<UnboundedReceiver<String>>>>,
    theme_mode: ThemeMode,
    active_patina: PatinaVariant,
    icon_handles: HashMap<String, image::Handle>,
    icons_requested: HashSet<String>,
    detail_screenshots: HashMap<String, Vec<(image::Handle, Option<String>)>>,
    detail_homepage: HashMap<String, String>,
    selected_screenshot_idx: usize,
    detail_loading: bool,
}

impl Default for CortenForgeApp {
    fn default() -> Self {
        let active_patina = ConfigManager::<AppearanceConfig>::load_or_default("appearance")
            .map(|c| PatinaVariant::from_code(&c.get().patina))
            .unwrap_or(PatinaVariant::Rust);

        Self {
            current_tab: NavigationTab::Discover,
            source_filter: SourceFilter::All,
            search_query: String::new(),
            catalog: Vec::new(),
            installed: Vec::new(),
            updates: Vec::new(),
            search_results: None,
            selected_package: None,
            active_logs: Vec::new(),
            is_busy: true,
            busy_message: Some("Yazılım kataloğu ve sistem paketleri taranıyor...".into()),
            log_rx: None,
            theme_mode: ThemeMode::Dark,
            active_patina,
            icon_handles: HashMap::new(),
            icons_requested: HashSet::new(),
            detail_screenshots: HashMap::new(),
            detail_homepage: HashMap::new(),
            selected_screenshot_idx: 0,
            detail_loading: false,
        }
    }
}

impl CortenForgeApp {
    fn new() -> (Self, Task<Message>) {
        let app = Self::default();
        let init_task = Task::batch(vec![
            Task::perform(
                PackageManager::fetch_curated_catalog(),
                Message::CatalogLoaded,
            ),
            Task::perform(
                PackageManager::fetch_flathub_collection(ProductCategory::Featured),
                Message::CategoryCatalogLoaded,
            ),
            Task::perform(
                PackageManager::fetch_installed_flatpaks(),
                Message::InstalledLoaded,
            ),
            Task::perform(
                PackageManager::check_flatpak_updates(),
                Message::UpdatesLoaded,
            ),
        ]);
        (app, init_task)
    }

    fn queue_icons_for(&mut self, items: &[PackageItem]) -> Task<Message> {
        let mut pending = Vec::new();
        for item in items {
            if self.icons_requested.insert(item.id.clone()) {
                pending.push((item.id.clone(), item.icon_url.clone(), item.source));
            }
        }
        if pending.is_empty() {
            Task::none()
        } else {
            Task::perform(
                PackageManager::fetch_icons_batch(pending),
                Message::IconsLoaded,
            )
        }
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::CatalogLoaded(items) => {
                let icon_task = self.queue_icons_for(&items);
                for item in items {
                    if !self.catalog.iter().any(|existing| existing.id == item.id) {
                        self.catalog.push(item);
                    }
                }
                self.is_busy = false;
                self.busy_message = None;
                return icon_task;
            }
            Message::CategoryCatalogLoaded(items) => {
                let icon_task = self.queue_icons_for(&items);
                for item in items {
                    if let Some(existing) = self.catalog.iter_mut().find(|e| e.id == item.id) {
                        if existing.icon_url.is_none() && item.icon_url.is_some() {
                            existing.icon_url = item.icon_url;
                        }
                    } else {
                        self.catalog.push(item);
                    }
                }
                self.is_busy = false;
                self.busy_message = None;
                return icon_task;
            }
            Message::InstalledLoaded(items) => {
                let icon_task = self.queue_icons_for(&items);
                self.installed = items;
                return icon_task;
            }
            Message::UpdatesLoaded(items) => {
                let icon_task = self.queue_icons_for(&items);
                self.updates = items;
                return icon_task;
            }
            Message::IconsLoaded(icons) => {
                for (app_id, bytes) in icons {
                    self.icon_handles
                        .insert(app_id, image::Handle::from_bytes(bytes));
                }
            }
            Message::AppDetailsLoaded(opt_details) => {
                self.detail_loading = false;
                if let Some(details) = opt_details {
                    let app_id = details.app_id.clone();
                    let apply_details = |pkg: &mut PackageItem| {
                        if let Some(ref desc) = details.description {
                            pkg.description = desc.clone();
                        }
                        if let Some(ref ver) = details.version {
                            pkg.version = ver.clone();
                        }
                        if let Some(ref dev) = details.developer {
                            pkg.developer = Some(dev.clone());
                        }
                        if let Some(ref lic) = details.license {
                            pkg.license = Some(lic.clone());
                        }
                        if let Some(ref dl) = details.download_size {
                            pkg.download_size = Some(dl.clone());
                        }
                        if let Some(ref inst) = details.installed_size {
                            pkg.installed_size = Some(inst.clone());
                        }
                        if let Some(ref url) = details.icon_url {
                            pkg.icon_url = Some(url.clone());
                        }
                    };
                    if let Some(ref mut selected) = self.selected_package
                        && selected.id == app_id
                    {
                        apply_details(selected);
                    }
                    for item in &mut self.catalog {
                        if item.id == app_id {
                            apply_details(item);
                        }
                    }
                    if let Some(ref mut results) = self.search_results {
                        for item in results.iter_mut() {
                            if item.id == app_id {
                                apply_details(item);
                            }
                        }
                    }
                    if let Some(homepage) = details.homepage {
                        self.detail_homepage.insert(app_id.clone(), homepage);
                    }

                    let mut tasks = Vec::new();
                    if !details.screenshots.is_empty()
                        && !self.detail_screenshots.contains_key(&app_id)
                    {
                        let sc_id = app_id.clone();
                        let sc_urls = details.screenshots;
                        tasks.push(Task::perform(
                            PackageManager::fetch_screenshots(sc_id, sc_urls),
                            |(id, imgs)| Message::ScreenshotsLoaded(id, imgs),
                        ));
                    }
                    if details.icon_url.is_some() && !self.icon_handles.contains_key(&app_id) {
                        let ic_id = app_id;
                        let ic_url = details.icon_url;
                        tasks.push(Task::perform(
                            PackageManager::fetch_icons_batch(vec![(
                                ic_id,
                                ic_url,
                                PackageSource::Flatpak,
                            )]),
                            Message::IconsLoaded,
                        ));
                    }
                    if !tasks.is_empty() {
                        return Task::batch(tasks);
                    }
                }
            }
            Message::ScreenshotsLoaded(app_id, imgs) => {
                let handles: Vec<(image::Handle, Option<String>)> = imgs
                    .into_iter()
                    .map(|(bytes, cap)| (image::Handle::from_bytes(bytes), cap))
                    .collect();
                if !handles.is_empty() {
                    self.detail_screenshots.insert(app_id, handles);
                }
            }
            Message::SelectScreenshot(idx) => {
                self.selected_screenshot_idx = idx;
            }
            Message::OpenHomepage(url) => {
                let _ = std::process::Command::new("xdg-open").arg(&url).spawn();
            }
            Message::PermissionsLoaded(app_id, opt_perm) => {
                if let Some(ref mut selected) = self.selected_package
                    && selected.id == app_id
                {
                    selected.permissions = opt_perm.clone();
                }
                for item in &mut self.catalog {
                    if item.id == app_id {
                        item.permissions = opt_perm.clone();
                    }
                }
            }
            Message::NavigateTo(tab) => {
                self.current_tab = tab.clone();
                if let NavigationTab::Categories(cat) = tab {
                    return Task::perform(
                        PackageManager::fetch_flathub_collection(cat),
                        Message::CategoryCatalogLoaded,
                    );
                }
            }
            Message::SetSourceFilter(filter) => {
                self.source_filter = filter;
            }
            Message::SearchInputChanged(query) => {
                self.search_query = query;
                if self.search_query.trim().is_empty() {
                    self.search_results = None;
                }
            }
            Message::PerformSearch => {
                let q = self.search_query.trim().to_string();
                if !q.is_empty() {
                    self.is_busy = true;
                    self.busy_message = Some(format!("'{}' için paketler aranıyor...", q));
                    return Task::perform(
                        async move { PackageManager::search_packages(&q).await },
                        Message::SearchResultsLoaded,
                    );
                }
            }
            Message::SearchResultsLoaded(results) => {
                let icon_task = self.queue_icons_for(&results);
                self.search_results = Some(results);
                self.is_busy = false;
                self.busy_message = None;
                return icon_task;
            }
            Message::OpenPackageDetail(pkg) => {
                let id = pkg.id.clone();
                let source = pkg.source;
                let is_flatpak = source == PackageSource::Flatpak;
                let icon_task = self.queue_icons_for(std::slice::from_ref(&pkg));
                self.selected_package = Some(pkg);
                self.current_tab = NavigationTab::Detail(id.clone());
                self.selected_screenshot_idx = 0;
                self.detail_loading = true;

                let mut tasks = vec![icon_task];
                let detail_id = id.clone();
                tasks.push(Task::perform(
                    async move { PackageManager::fetch_app_details(&detail_id, source).await },
                    Message::AppDetailsLoaded,
                ));
                if is_flatpak {
                    tasks.push(Task::perform(
                        async move {
                            let perm = PackageManager::fetch_sandbox_permissions(&id).await;
                            (id, perm)
                        },
                        |(id, perm)| Message::PermissionsLoaded(id, perm),
                    ));
                }
                return Task::batch(tasks);
            }
            Message::InstallRequested(pkg) => {
                self.is_busy = true;
                self.busy_message = Some(format!("{} kuruluyor...", pkg.name));
                self.active_logs.clear();
                self.active_logs
                    .push(format!(">>> {} kurulumu başlatıldı...\n", pkg.name));

                let (tx, rx) = unbounded_channel();
                self.log_rx = Some(Arc::new(Mutex::new(rx)));

                let pkg_clone = pkg.clone();
                return Task::perform(
                    async move { PackageManager::install_package(pkg_clone, tx).await },
                    Message::OperationFinished,
                );
            }
            Message::UninstallRequested(pkg) => {
                self.is_busy = true;
                self.busy_message = Some(format!("{} kaldırılıyor...", pkg.name));
                self.active_logs.clear();
                self.active_logs
                    .push(format!(">>> {} kaldırma işlemi başlatıldı...\n", pkg.name));

                let (tx, rx) = unbounded_channel();
                self.log_rx = Some(Arc::new(Mutex::new(rx)));

                let pkg_clone = pkg.clone();
                return Task::perform(
                    async move { PackageManager::uninstall_package(pkg_clone, tx).await },
                    Message::OperationFinished,
                );
            }
            Message::LaunchApp(pkg) => {
                let x11_display = std::env::var("DISPLAY").ok().or_else(|| {
                    std::env::var("XDG_RUNTIME_DIR")
                        .ok()
                        .and_then(|d| {
                            std::fs::read_to_string(
                                std::path::PathBuf::from(d).join("corten-x11-display"),
                            )
                            .ok()
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
                match pkg.source {
                    PackageSource::Flatpak => {
                        let mut cmd = std::process::Command::new("flatpak");
                        cmd.args(["run", &pkg.id]);
                        if let Some(ref disp) = x11_display {
                            cmd.env("DISPLAY", disp);
                        }
                        let _ = cmd.spawn();
                    }
                    PackageSource::ArchRepo | PackageSource::Aur => {
                        let mut cmd = std::process::Command::new(&pkg.id);
                        if let Some(ref disp) = x11_display {
                            cmd.env("DISPLAY", disp);
                        }
                        let _ = cmd.spawn();
                    }
                    PackageSource::CortenCore => {}
                }
            }
            Message::LogLineReceived(line) => {
                self.active_logs.push(line);
            }
            Message::OperationFinished(res) => {
                self.is_busy = false;
                self.busy_message = None;
                self.log_rx = None;
                match res {
                    Ok(()) => {
                        self.active_logs
                            .push("\n>>> İşlem başarıyla tamamlandı.\n".into());
                    }
                    Err(err) => {
                        self.active_logs.push(format!("\n>>> Hata: {}\n", err));
                    }
                }
                // Refresh catalog and installed list
                return Task::done(Message::RefreshAll);
            }
            Message::RefreshAll => {
                self.is_busy = true;
                self.busy_message = Some("Sistem paketleri yenileniyor...".into());
                return Task::batch(vec![
                    Task::perform(
                        PackageManager::fetch_curated_catalog(),
                        Message::CatalogLoaded,
                    ),
                    Task::perform(
                        PackageManager::fetch_installed_flatpaks(),
                        Message::InstalledLoaded,
                    ),
                    Task::perform(
                        PackageManager::check_flatpak_updates(),
                        Message::UpdatesLoaded,
                    ),
                ]);
            }
            Message::EventTriggered(_) => {}
        }
        Task::none()
    }

    fn view<'a>(&'a self) -> Element<'a, Message> {
        let palette = Palette::for_mode(self.theme_mode).with_patina(self.active_patina);

        // Sidebar Navigation
        let sidebar = self.render_sidebar(palette);

        // Top Search & Filter Bar
        let top_bar = self.render_top_bar(palette);

        // Main Viewport Content
        let content: Element<'_, Message> = match &self.current_tab {
            NavigationTab::Discover => self.render_discover_view(palette),
            NavigationTab::Categories(cat) => self.render_category_view(*cat, palette),
            NavigationTab::Installed => self.render_installed_view(palette),
            NavigationTab::Updates => self.render_updates_view(palette),
            NavigationTab::Detail(_) => {
                if let Some(ref pkg) = self.selected_package {
                    self.render_detail_view(pkg, palette)
                } else {
                    self.render_discover_view(palette)
                }
            }
        };

        let mut right_col = column![top_bar, content]
            .width(Length::Fill)
            .height(Length::Fill)
            .spacing(Spacing::MD);

        if self.is_busy || self.busy_message.is_some() || !self.active_logs.is_empty() {
            let last_line = self
                .active_logs
                .iter()
                .rev()
                .find(|l| !l.trim().is_empty())
                .map(|l| l.trim().to_string());
            let status_text = if let Some(ref msg) = self.busy_message {
                if let Some(ref line) = last_line {
                    format!("{} — {}", msg, line)
                } else {
                    msg.clone()
                }
            } else {
                last_line.unwrap_or_default()
            };
            let is_err = status_text.contains("Hata:") || status_text.contains("başarısız");
            let accent = if is_err {
                palette.danger.to_iced()
            } else if self.is_busy {
                palette.primary.to_iced()
            } else {
                palette.success.to_iced()
            };
            let status_bar = container(
                row![
                    text(if self.is_busy {
                        Icon::REFRESH
                    } else {
                        Icon::CHECK
                    })
                    .font(LUCIDE_FONT)
                    .size(13.0)
                    .color(accent),
                    text(status_text)
                        .size(11.5)
                        .color(palette.text_primary.to_iced()),
                ]
                .spacing(Spacing::SM)
                .align_y(Alignment::Center),
            )
            .width(Length::Fill)
            .padding([8, 12])
            .style(move |_| container::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgba(
                    0.09, 0.10, 0.13, 0.96,
                ))),
                border: iced::Border {
                    color: accent,
                    width: 1.0,
                    radius: iced::border::Radius::from(Radius::MD),
                },
                ..Default::default()
            });
            right_col = right_col.push(status_bar);
        }

        let right_pane = right_col;

        container(
            row![sidebar, right_pane]
                .spacing(Spacing::LG)
                .padding(Spacing::LG),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .style(move |_| container::Style {
            background: Some(iced::Background::Color(iced::Color::from_rgba(
                0.06, 0.07, 0.09, 0.98,
            ))),
            ..Default::default()
        })
        .into()
    }

    fn subscription(&self) -> Subscription<Message> {
        let events = event::listen().map(Message::EventTriggered);
        let mut subs = vec![events];

        if let Some(rx_arc) = &self.log_rx {
            let rx_clone = Arc::clone(rx_arc);
            let log_stream = futures::stream::unfold(rx_clone, |rx| async move {
                let mut lock = rx.lock().await;
                let line = lock.recv().await?;
                drop(lock);
                Some((Message::LogLineReceived(line), rx))
            });
            subs.push(Subscription::run_with_id("corten-forge-logs", log_stream));
        }

        Subscription::batch(subs)
    }

    // ─────────────────────────────────────────────────────────────────────────────
    // UI SUB-COMPONENTS
    // ─────────────────────────────────────────────────────────────────────────────

    fn render_sidebar<'a>(&'a self, palette: Palette) -> Element<'a, Message> {
        // App Branding
        let brand_block = column![
            row![
                text(Icon::LAYERS)
                    .font(LUCIDE_FONT)
                    .size(Typography::TITLE_SM)
                    .color(palette.primary.to_iced()),
                text("CORTEN FORGE")
                    .size(Typography::TITLE_SM)
                    .color(palette.text_primary.to_iced()),
            ]
            .spacing(Spacing::XS)
            .align_y(Alignment::Center),
            text("Wayland Yazılım & Paket Merkezi")
                .size(11.0)
                .color(palette.text_muted.to_iced()),
        ]
        .spacing(Spacing::XXS);

        // Navigation Items
        let nav_discover = Self::sidebar_nav_button(
            Icon::SPARKLES,
            "Keşfet (Vitrin)",
            None,
            matches!(self.current_tab, NavigationTab::Discover),
            palette,
            Message::NavigateTo(NavigationTab::Discover),
        );

        let nav_installed = Self::sidebar_nav_button(
            Icon::DOWNLOAD,
            "Yüklü Uygulamalar",
            Some(self.installed.len()),
            matches!(self.current_tab, NavigationTab::Installed),
            palette,
            Message::NavigateTo(NavigationTab::Installed),
        );

        let nav_updates = Self::sidebar_nav_button(
            Icon::REFRESH,
            "Güncellemeler",
            if self.updates.is_empty() {
                None
            } else {
                Some(self.updates.len())
            },
            matches!(self.current_tab, NavigationTab::Updates),
            palette,
            Message::NavigateTo(NavigationTab::Updates),
        );

        // Categories Group
        let cat_label = text("KATEGORİLER")
            .size(10.0)
            .color(palette.text_muted.to_iced());

        let categories_col = column![
            cat_label,
            Self::category_nav_item(ProductCategory::Development, &self.current_tab, palette),
            Self::category_nav_item(ProductCategory::Graphics, &self.current_tab, palette),
            Self::category_nav_item(ProductCategory::AudioVideo, &self.current_tab, palette),
            Self::category_nav_item(ProductCategory::Productivity, &self.current_tab, palette),
            Self::category_nav_item(ProductCategory::System, &self.current_tab, palette),
            Self::category_nav_item(ProductCategory::Gaming, &self.current_tab, palette),
            Self::category_nav_item(ProductCategory::Internet, &self.current_tab, palette),
        ]
        .spacing(Spacing::XS);

        // Bottom Storage / System Info Box
        let sys_info = container(
            column![
                row![
                    text("Sistem Durumu")
                        .size(11.0)
                        .color(palette.text_primary.to_iced()),
                    horizontal_space(),
                    text("Çevrimiçi")
                        .size(10.0)
                        .color(palette.primary.to_iced()),
                ]
                .align_y(Alignment::Center),
                text("Flathub & Arch Resmi Depoları")
                    .size(10.0)
                    .color(palette.text_muted.to_iced()),
            ]
            .spacing(4),
        )
        .padding([8, 10])
        .style(move |_| container::Style {
            background: Some(iced::Background::Color(iced::Color::from_rgba(
                1.0, 1.0, 1.0, 0.03,
            ))),
            border: iced::Border {
                color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.06),
                width: 1.0,
                radius: iced::border::Radius::from(Radius::SM),
            },
            ..Default::default()
        });

        container(
            column![
                brand_block,
                vertical_space().height(Spacing::MD),
                nav_discover,
                nav_installed,
                nav_updates,
                vertical_space().height(Spacing::MD),
                categories_col,
                vertical_space(),
                sys_info,
            ]
            .spacing(Spacing::XS),
        )
        .width(Length::Fixed(240.0))
        .height(Length::Fill)
        .padding(Spacing::MD)
        .style(move |_| container::Style {
            background: Some(iced::Background::Color(iced::Color::from_rgba(
                0.08, 0.09, 0.12, 0.98,
            ))),
            border: iced::Border {
                color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.07),
                width: 1.0,
                radius: iced::border::Radius::from(Radius::LG),
            },
            ..Default::default()
        })
        .into()
    }

    fn render_top_bar<'a>(&'a self, palette: Palette) -> Element<'a, Message> {
        let is_detail = matches!(self.current_tab, NavigationTab::Detail(_));
        let back_btn: Element<'_, Message> = if is_detail {
            button(
                row![
                    text("←")
                        .size(Typography::BODY)
                        .color(palette.primary.to_iced()),
                    text("Geri")
                        .size(Typography::BODY_SM)
                        .color(palette.text_primary.to_iced()),
                ]
                .spacing(4)
                .align_y(Alignment::Center),
            )
            .padding([6, 12])
            .style(move |_, status| {
                let bg = match status {
                    button::Status::Hovered => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.10),
                    _ => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.04),
                };
                button::Style {
                    background: Some(iced::Background::Color(bg)),
                    border: iced::Border {
                        color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.08),
                        width: 1.0,
                        radius: iced::border::Radius::from(Radius::SM),
                    },
                    ..Default::default()
                }
            })
            .on_press(Message::NavigateTo(NavigationTab::Discover))
            .into()
        } else {
            horizontal_space().width(0).into()
        };

        let search_input = container(
            text_input(
                "Uygulama ara (Blender, Spotify, Neovim)...",
                &self.search_query,
            )
            .padding([8, 12])
            .on_input(Message::SearchInputChanged)
            .on_submit(Message::PerformSearch)
            .width(Length::Fill)
            .style(move |_, _| text_input::Style {
                background: iced::Background::Color(iced::Color::from_rgba(1.0, 1.0, 1.0, 0.04)),
                border: iced::Border {
                    color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.08),
                    width: 1.0,
                    radius: iced::border::Radius::from(Radius::SM),
                },
                icon: palette.primary.to_iced(),
                placeholder: palette.text_muted.to_iced(),
                value: palette.text_primary.to_iced(),
                selection: iced::Color::from_rgba(
                    palette.primary.r,
                    palette.primary.g,
                    palette.primary.b,
                    0.35,
                ),
            }),
        )
        .width(Length::Fill)
        .clip(true);

        let search_btn = button(
            row![
                text(Icon::SEARCH)
                    .font(LUCIDE_FONT)
                    .size(13.0)
                    .color(palette.on_primary().to_iced()),
                text("Ara")
                    .size(Typography::BODY_SM)
                    .color(palette.on_primary().to_iced()),
            ]
            .spacing(5)
            .align_y(Alignment::Center),
        )
        .padding([8, 14])
        .style(move |_, _| button::Style {
            background: Some(iced::Background::Color(palette.primary.to_iced())),
            border: iced::Border {
                radius: iced::border::Radius::from(Radius::SM),
                ..Default::default()
            },
            ..Default::default()
        })
        .on_press(Message::PerformSearch);

        // Filter Pills: All | Flatpak | Arch
        let filter_all =
            Self::source_filter_chip("Tümü", SourceFilter::All, self.source_filter, palette);
        let filter_flatpak = Self::source_filter_chip(
            "Flatpak",
            SourceFilter::FlatpakOnly,
            self.source_filter,
            palette,
        );
        let filter_arch =
            Self::source_filter_chip("Arch", SourceFilter::ArchOnly, self.source_filter, palette);

        let refresh_btn = button(
            row![
                text(Icon::REFRESH)
                    .font(LUCIDE_FONT)
                    .size(12.0)
                    .color(palette.text_muted.to_iced()),
                text("Yenile")
                    .size(Typography::CAPTION)
                    .color(palette.text_muted.to_iced()),
            ]
            .spacing(5)
            .align_y(Alignment::Center),
        )
        .padding([6, 10])
        .style(move |_, status| {
            let bg = match status {
                button::Status::Hovered => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.10),
                _ => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.04),
            };
            button::Style {
                background: Some(iced::Background::Color(bg)),
                border: iced::Border {
                    color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.08),
                    width: 1.0,
                    radius: iced::border::Radius::from(Radius::SM),
                },
                ..Default::default()
            }
        })
        .on_press(Message::RefreshAll);

        container(
            row![
                back_btn,
                search_input,
                search_btn,
                row![filter_all, filter_flatpak, filter_arch].spacing(Spacing::XS),
                refresh_btn,
            ]
            .spacing(Spacing::SM)
            .align_y(Alignment::Center),
        )
        .padding([8, 12])
        .width(Length::Fill)
        .style(move |_| container::Style {
            background: Some(iced::Background::Color(iced::Color::from_rgba(
                0.08, 0.09, 0.12, 0.95,
            ))),
            border: iced::Border {
                color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.07),
                width: 1.0,
                radius: iced::border::Radius::from(Radius::MD),
            },
            ..Default::default()
        })
        .into()
    }

    fn render_discover_view<'a>(&'a self, palette: Palette) -> Element<'a, Message> {
        let mut col = column![].spacing(Spacing::LG);

        // If there are search results, show them first
        if let Some(ref results) = self.search_results {
            col = col.push(
                row![
                    text(format!("Arama Sonuçları: \"{}\"", self.search_query))
                        .size(Typography::TITLE_SM)
                        .color(palette.text_primary.to_iced()),
                    horizontal_space(),
                    text(format!("{} paket bulundu", results.len()))
                        .size(11.0)
                        .color(palette.text_muted.to_iced()),
                ]
                .align_y(Alignment::Center),
            );

            let grid = self.render_package_grid(results, palette);
            col = col.push(grid);
            return scrollable(col).height(Length::Fill).into();
        }

        // Hero Spotlight Banner
        let hero = container(
            row![
                column![
                    container(
                        text("HAFTANIN SEÇİMİ")
                            .size(10.0)
                            .color(palette.primary.to_iced()),
                    )
                    .padding([2, 6])
                    .style(move |_| container::Style {
                        background: Some(iced::Background::Color(iced::Color::from_rgba(
                            palette.primary.r,
                            palette.primary.g,
                            palette.primary.b,
                            0.15,
                        ))),
                        border: iced::Border {
                            radius: iced::border::Radius::from(Radius::SM),
                            color: palette.primary.to_iced(),
                            width: 1.0,
                        },
                        ..Default::default()
                    }),
                    text("Blender 3D — Yeni Nesil Yaratıcı Stüdyo")
                        .size(Typography::TITLE)
                        .color(palette.text_primary.to_iced()),
                    text("3D modelleme, VFX, heykel ve gerçek zamanlı render için endüstri standardı açık kaynaklı güç. Flathub üzerinden tam sanal alan (sandbox) izolasyonu ile tek tıkla kurun.")
                        .size(Typography::BODY_SM)
                        .color(palette.text_muted.to_iced()),
                    vertical_space().height(4),
                    row![
                        button(
                            text("Detayları ve Güvenliği İncele →")
                                .size(Typography::BODY_SM)
                                .color(palette.text_primary.to_iced()),
                        )
                        .padding([8, 16])
                        .style(move |_, _| button::Style {
                            background: Some(iced::Background::Color(palette.primary.to_iced())),
                            border: iced::Border {
                                radius: iced::border::Radius::from(Radius::SM),
                                ..Default::default()
                            },
                            ..Default::default()
                        })
                        .on_press(Message::OpenPackageDetail(PackageItem {
                            id: "org.blender.Blender".into(),
                            name: "Blender 3D".into(),
                            summary: "Özgür ve açık kaynaklı 3D modelleme & animasyon kiti".into(),
                            description: "Blender 3D üretim hattının tamamını destekleyen açık kaynaklı bir grafik paketidir.".into(),
                            version: "5.2.0".into(),
                            source: PackageSource::Flatpak,
                            state: PackageState::Available,
                            category: ProductCategory::Graphics,
                            icon_char: crate::package::ICON_APP,
                            icon_url: None,
                            download_size: Some("320 MB".into()),
                            installed_size: Some("890 MB".into()),
                            developer: Some("Blender Foundation".into()),
                            license: Some("GPL-3.0".into()),
                            permissions: None,
                        })),
                    ]
                    .align_y(Alignment::Center),
                ]
                .spacing(Spacing::XS)
                .width(Length::FillPortion(3)),
                self.render_app_icon("org.blender.Blender", crate::package::ICON_APP, 84.0, palette),
            ]
            .spacing(Spacing::LG)
            .align_y(Alignment::Center),
        )
        .padding(Spacing::LG)
        .width(Length::Fill)
        .style(move |_| container::Style {
            background: Some(iced::Background::Color(iced::Color::from_rgba(
                palette.primary.r,
                palette.primary.g,
                palette.primary.b,
                0.08,
            ))),
            border: iced::Border {
                color: palette.primary.to_iced(),
                width: 1.5,
                radius: iced::border::Radius::from(Radius::LG),
            },
            ..Default::default()
        });

        col = col.push(hero);

        // Filter catalog items according to source filter
        let filtered_items: Vec<PackageItem> = self
            .catalog
            .iter()
            .filter(|p| match self.source_filter {
                SourceFilter::All => true,
                SourceFilter::FlatpakOnly => p.source == PackageSource::Flatpak,
                SourceFilter::ArchOnly => {
                    p.source == PackageSource::ArchRepo || p.source == PackageSource::Aur
                }
            })
            .cloned()
            .collect();

        // Section: Popüler Masaüstü Araçları
        col = col.push(
            text("Öne Çıkan ve Popüler Uygulamalar")
                .size(Typography::TITLE_SM)
                .color(palette.text_primary.to_iced()),
        );

        let grid = self.render_package_grid(&filtered_items, palette);
        col = col.push(grid);

        scrollable(col).height(Length::Fill).into()
    }

    fn render_category_view<'a>(
        &'a self,
        category: ProductCategory,
        palette: Palette,
    ) -> Element<'a, Message> {
        let items: Vec<PackageItem> = self
            .catalog
            .iter()
            .filter(|p| p.category == category || category == ProductCategory::All)
            .filter(|p| match self.source_filter {
                SourceFilter::All => true,
                SourceFilter::FlatpakOnly => p.source == PackageSource::Flatpak,
                SourceFilter::ArchOnly => p.source == PackageSource::ArchRepo,
            })
            .cloned()
            .collect();

        let header = row![
            text(category.icon()).size(Typography::TITLE),
            column![
                text(category.label())
                    .size(Typography::TITLE_SM)
                    .color(palette.text_primary.to_iced()),
                text(format!("{} adet uygulama listeleniyor", items.len()))
                    .size(11.0)
                    .color(palette.text_muted.to_iced()),
            ]
            .spacing(2),
        ]
        .spacing(Spacing::SM)
        .align_y(Alignment::Center);

        let grid = self.render_package_grid(&items, palette);

        scrollable(column![header, vertical_space().height(Spacing::XS), grid].spacing(Spacing::MD))
            .height(Length::Fill)
            .into()
    }

    fn render_installed_view<'a>(&'a self, palette: Palette) -> Element<'a, Message> {
        let count_text = format!("{} adet yerel uygulama kurulu", self.installed.len());
        let header = row![
            row![
                text(Icon::DOWNLOAD)
                    .font(LUCIDE_FONT)
                    .size(Typography::TITLE_SM)
                    .color(palette.primary.to_iced()),
                text("Yüklü Uygulamalar")
                    .size(Typography::TITLE_SM)
                    .color(palette.text_primary.to_iced()),
            ]
            .spacing(Spacing::XS)
            .align_y(Alignment::Center),
            horizontal_space(),
            text(count_text)
                .size(11.0)
                .color(palette.text_muted.to_iced()),
        ]
        .align_y(Alignment::Center);

        let mut list_col = column![].spacing(Spacing::XS);

        for item in &self.installed {
            let item_clone = item.clone();
            let item_clone2 = item.clone();

            let row_entry = container(
                row![
                    self.render_app_icon(&item.id, item.icon_char, 34.0, palette),
                    column![
                        text(item.name.clone())
                            .size(Typography::BODY)
                            .color(palette.text_primary.to_iced()),
                        text(item.id.clone())
                            .size(11.0)
                            .color(palette.text_muted.to_iced()),
                    ]
                    .spacing(2)
                    .width(Length::FillPortion(3)),
                    container(
                        text(format!("v{}", item.version))
                            .size(11.0)
                            .color(palette.text_muted.to_iced())
                    )
                    .padding([2, 6]),
                    button(
                        text("Detay / Güvenlik")
                            .size(11.0)
                            .color(palette.text_primary.to_iced()),
                    )
                    .padding([5, 10])
                    .style(move |_, status| {
                        let bg = match status {
                            button::Status::Hovered => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.10),
                            _ => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.04),
                        };
                        button::Style {
                            background: Some(iced::Background::Color(bg)),
                            border: iced::Border {
                                color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.08),
                                width: 1.0,
                                radius: iced::border::Radius::from(Radius::SM),
                            },
                            ..Default::default()
                        }
                    })
                    .on_press(Message::OpenPackageDetail(item_clone)),
                    button(
                        text("Kaldır")
                            .size(11.0)
                            .color(iced::Color::from_rgb(0.95, 0.35, 0.30)),
                    )
                    .padding([5, 10])
                    .style(move |_, status| {
                        let bg = match status {
                            button::Status::Hovered => iced::Color::from_rgba(0.9, 0.25, 0.2, 0.25),
                            _ => iced::Color::from_rgba(0.9, 0.25, 0.2, 0.10),
                        };
                        button::Style {
                            background: Some(iced::Background::Color(bg)),
                            border: iced::Border {
                                color: iced::Color::from_rgba(0.9, 0.25, 0.2, 0.30),
                                width: 1.0,
                                radius: iced::border::Radius::from(Radius::SM),
                            },
                            ..Default::default()
                        }
                    })
                    .on_press(Message::UninstallRequested(item_clone2)),
                ]
                .spacing(Spacing::MD)
                .align_y(Alignment::Center),
            )
            .padding([8, 12])
            .width(Length::Fill)
            .style(move |_| container::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgba(
                    1.0, 1.0, 1.0, 0.025,
                ))),
                border: iced::Border {
                    color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.06),
                    width: 1.0,
                    radius: iced::border::Radius::from(Radius::SM),
                },
                ..Default::default()
            });

            list_col = list_col.push(row_entry);
        }

        scrollable(column![header, list_col].spacing(Spacing::MD))
            .height(Length::Fill)
            .into()
    }

    fn render_updates_view<'a>(&'a self, palette: Palette) -> Element<'a, Message> {
        let is_up_to_date = self.updates.is_empty();

        let header = row![
            row![
                text(Icon::REFRESH)
                    .font(LUCIDE_FONT)
                    .size(Typography::TITLE_SM)
                    .color(palette.primary.to_iced()),
                text("Sistem & Uygulama Güncellemeleri")
                    .size(Typography::TITLE_SM)
                    .color(palette.text_primary.to_iced()),
            ]
            .spacing(Spacing::XS)
            .align_y(Alignment::Center),
            horizontal_space(),
            if is_up_to_date {
                text("✓ Tüm uygulamalarınız güncel")
                    .size(Typography::BODY_SM)
                    .color(palette.success.to_iced())
            } else {
                text(format!("{} bekleyen güncelleme", self.updates.len()))
                    .size(Typography::BODY_SM)
                    .color(palette.warning.to_iced())
            },
        ]
        .align_y(Alignment::Center);

        let update_all_btn = if !is_up_to_date {
            button(
                text("Tümünü Güncelle")
                    .size(Typography::BODY_SM)
                    .color(palette.text_primary.to_iced()),
            )
            .padding([8, 16])
            .style(move |_, _| button::Style {
                background: Some(iced::Background::Color(palette.primary.to_iced())),
                border: iced::Border {
                    radius: iced::border::Radius::from(Radius::SM),
                    ..Default::default()
                },
                ..Default::default()
            })
            .on_press(Message::RefreshAll)
        } else {
            button(
                text("Güncellemeleri Kontrol Et")
                    .size(Typography::BODY_SM)
                    .color(palette.text_muted.to_iced()),
            )
            .padding([8, 16])
            .style(move |_, _| button::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgba(
                    1.0, 1.0, 1.0, 0.05,
                ))),
                border: iced::Border {
                    radius: iced::border::Radius::from(Radius::SM),
                    ..Default::default()
                },
                ..Default::default()
            })
            .on_press(Message::RefreshAll)
        };

        // Live Log Terminal Console
        let mut log_lines = column![].spacing(2);
        if self.active_logs.is_empty() {
            log_lines = log_lines.push(
                text("İşlem kaydı yok. Kurulum veya güncelleme yapıldığında canlı akış burada görüntülenecektir.")
                    .size(11.0)
                    .color(palette.text_muted.to_iced()),
            );
        } else {
            for line in &self.active_logs {
                log_lines =
                    log_lines.push(text(line).size(11.0).color(palette.text_primary.to_iced()));
            }
        }

        let terminal_box = container(
            column![
                row![
                    text("CANLI TERMİNAL GÜNLÜĞÜ")
                        .size(10.0)
                        .color(palette.text_muted.to_iced()),
                    horizontal_space(),
                    if self.is_busy {
                        text("İşleniyor...")
                            .size(10.0)
                            .color(palette.warning.to_iced())
                    } else {
                        text("Boşta").size(10.0).color(palette.text_muted.to_iced())
                    },
                ]
                .align_y(Alignment::Center),
                scrollable(log_lines).height(Length::Fixed(200.0)),
            ]
            .spacing(Spacing::XS),
        )
        .padding(Spacing::MD)
        .width(Length::Fill)
        .style(move |_| container::Style {
            background: Some(iced::Background::Color(iced::Color::from_rgba(
                0.03, 0.04, 0.05, 0.98,
            ))),
            border: iced::Border {
                color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.08),
                width: 1.0,
                radius: iced::border::Radius::from(Radius::MD),
            },
            ..Default::default()
        });

        scrollable(
            column![
                header,
                update_all_btn,
                vertical_space().height(Spacing::SM),
                terminal_box,
            ]
            .spacing(Spacing::MD),
        )
        .height(Length::Fill)
        .into()
    }

    fn render_detail_view<'a>(
        &'a self,
        pkg: &'a PackageItem,
        palette: Palette,
    ) -> Element<'a, Message> {
        let pkg_clone = pkg.clone();
        let pkg_clone2 = pkg.clone();
        let is_installed = pkg.state == PackageState::Installed;

        // Big App Header
        let (r, g, b) = pkg.source.badge_color();
        let source_badge = container(
            text(pkg.source.label())
                .size(10.5)
                .color(iced::Color::from_rgb(r, g, b)),
        )
        .padding([3, 8])
        .style(move |_| container::Style {
            background: Some(iced::Background::Color(iced::Color::from_rgba(
                r, g, b, 0.15,
            ))),
            border: iced::Border {
                color: iced::Color::from_rgb(r, g, b),
                width: 1.0,
                radius: iced::border::Radius::from(Radius::SM),
            },
            ..Default::default()
        });

        let pkg_clone3 = pkg.clone();
        let action_btn: Element<'a, Message> = if is_installed {
            let launch_btn = button(
                row![
                    text(Icon::PLAY)
                        .font(LUCIDE_FONT)
                        .size(13.0)
                        .color(palette.on_primary().to_iced()),
                    text("Aç")
                        .size(Typography::BODY)
                        .color(palette.on_primary().to_iced()),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
            )
            .padding([10, 20])
            .style(move |_, _| button::Style {
                background: Some(iced::Background::Color(palette.primary.to_iced())),
                border: iced::Border {
                    radius: iced::border::Radius::from(Radius::MD),
                    ..Default::default()
                },
                ..Default::default()
            })
            .on_press(Message::LaunchApp(pkg_clone3));

            let remove_btn = button(
                text("Kaldır")
                    .size(Typography::BODY)
                    .color(iced::Color::from_rgb(0.95, 0.35, 0.30)),
            )
            .padding([10, 16])
            .style(move |_, status| {
                let bg = match status {
                    button::Status::Hovered => iced::Color::from_rgba(0.9, 0.25, 0.2, 0.25),
                    _ => iced::Color::from_rgba(0.9, 0.25, 0.2, 0.12),
                };
                button::Style {
                    background: Some(iced::Background::Color(bg)),
                    border: iced::Border {
                        color: iced::Color::from_rgba(0.9, 0.25, 0.2, 0.40),
                        width: 1.0,
                        radius: iced::border::Radius::from(Radius::MD),
                    },
                    ..Default::default()
                }
            })
            .on_press(Message::UninstallRequested(pkg_clone));

            row![launch_btn, remove_btn]
                .spacing(Spacing::XS)
                .align_y(Alignment::Center)
                .into()
        } else {
            button(
                text("Kurulum Yap")
                    .size(Typography::BODY)
                    .color(palette.on_primary().to_iced()),
            )
            .padding([10, 24])
            .style(move |_, _| button::Style {
                background: Some(iced::Background::Color(palette.primary.to_iced())),
                border: iced::Border {
                    radius: iced::border::Radius::from(Radius::MD),
                    ..Default::default()
                },
                ..Default::default()
            })
            .on_press(Message::InstallRequested(pkg_clone2))
            .into()
        };

        let app_header = row![
            self.render_app_icon(&pkg.id, pkg.icon_char, 64.0, palette),
            column![
                row![
                    text(pkg.name.clone())
                        .size(Typography::TITLE)
                        .color(palette.text_primary.to_iced()),
                    source_badge,
                ]
                .spacing(Spacing::SM)
                .align_y(Alignment::Center),
                text(pkg.summary.clone())
                    .size(Typography::BODY_SM)
                    .color(palette.text_muted.to_iced()),
                {
                    let clean_license = match pkg.license.as_deref() {
                        None => "Açık Kaynak".to_string(),
                        Some(l) if l.starts_with("LicenseRef-proprietary") => {
                            "Özel Mülk".to_string()
                        }
                        Some(l) if l.contains("http://") || l.contains("https://") => {
                            let base = l.split('=').next().unwrap_or(l).trim();
                            if base.is_empty() || base.starts_with("LicenseRef-") {
                                "Özel Lisans".to_string()
                            } else {
                                base.to_string()
                            }
                        }
                        Some(l) if l.chars().count() > 28 => {
                            format!("{}…", l.chars().take(27).collect::<String>())
                        }
                        Some(l) => l.to_string(),
                    };
                    let meta_row_1 = row![
                        text(format!("Sürüm: {}", pkg.version))
                            .size(11.0)
                            .color(palette.text_muted.to_iced()),
                        text("•").size(11.0).color(palette.text_muted.to_iced()),
                        text(pkg.developer.as_deref().unwrap_or("Topluluk"))
                            .size(11.0)
                            .color(palette.text_muted.to_iced()),
                        text("•").size(11.0).color(palette.text_muted.to_iced()),
                        text(clean_license)
                            .size(11.0)
                            .color(palette.text_muted.to_iced()),
                    ]
                    .spacing(Spacing::XS)
                    .align_y(Alignment::Center);

                    let mut meta_col = column![meta_row_1].spacing(3);
                    let mut meta_row_2 = row![].spacing(Spacing::SM).align_y(Alignment::Center);
                    let mut has_row_2 = false;
                    if let Some(ref dl) = pkg.download_size {
                        meta_row_2 = meta_row_2.push(
                            text(format!("İndirme: {}", dl))
                                .size(11.0)
                                .color(palette.text_muted.to_iced()),
                        );
                        has_row_2 = true;
                    }
                    if let Some(url) = self.detail_homepage.get(&pkg.id) {
                        meta_row_2 = meta_row_2.push(
                            button(
                                row![
                                    text(Icon::GLOBE)
                                        .font(LUCIDE_FONT)
                                        .size(11.0)
                                        .color(palette.primary.to_iced()),
                                    text("Web Sitesi")
                                        .size(11.0)
                                        .color(palette.primary.to_iced()),
                                ]
                                .spacing(4)
                                .align_y(Alignment::Center),
                            )
                            .padding([2, 6])
                            .style(move |_, status| button::Style {
                                background: Some(iced::Background::Color(match status {
                                    button::Status::Hovered => {
                                        iced::Color::from_rgba(1.0, 1.0, 1.0, 0.08)
                                    }
                                    _ => iced::Color::TRANSPARENT,
                                })),
                                border: iced::Border {
                                    radius: iced::border::Radius::from(Radius::SM),
                                    ..Default::default()
                                },
                                ..Default::default()
                            })
                            .on_press(Message::OpenHomepage(url.clone())),
                        );
                        has_row_2 = true;
                    }
                    if has_row_2 {
                        meta_col = meta_col.push(meta_row_2);
                    }
                    meta_col
                },
            ]
            .spacing(Spacing::XXS)
            .width(Length::Fill),
            action_btn,
        ]
        .spacing(Spacing::LG)
        .align_y(Alignment::Center);

        // Screenshot Gallery Section
        let screenshots_section: Option<Element<'a, Message>> = if let Some(shots) =
            self.detail_screenshots.get(&pkg.id)
        {
            if shots.is_empty() {
                None
            } else {
                let active_idx = self.selected_screenshot_idx.min(shots.len() - 1);
                let (active_handle, active_caption) = &shots[active_idx];

                let hero_img = container(
                    image(active_handle.clone())
                        .width(Length::Fill)
                        .height(Length::Fixed(320.0))
                        .content_fit(ContentFit::Contain),
                )
                .width(Length::Fill)
                .padding(Spacing::SM)
                .center_x(Length::Fill)
                .style(move |_| container::Style {
                    background: Some(iced::Background::Color(iced::Color::from_rgba(
                        0.0, 0.0, 0.0, 0.35,
                    ))),
                    border: iced::Border {
                        color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.08),
                        width: 1.0,
                        radius: iced::border::Radius::from(Radius::MD),
                    },
                    ..Default::default()
                });

                let mut gallery_col = column![
                    row![
                        text("Ekran Görüntüleri")
                            .size(Typography::TITLE_SM)
                            .color(palette.text_primary.to_iced()),
                        horizontal_space(),
                        text(format!("{} / {}", active_idx + 1, shots.len()))
                            .size(11.0)
                            .color(palette.text_muted.to_iced()),
                    ]
                    .align_y(Alignment::Center),
                    hero_img,
                ]
                .spacing(Spacing::SM);

                if let Some(cap) = active_caption {
                    gallery_col = gallery_col.push(
                        text(cap.clone())
                            .size(Typography::BODY_SM)
                            .color(palette.text_muted.to_iced()),
                    );
                }

                if shots.len() > 1 {
                    let mut thumbs = row![].spacing(Spacing::SM).align_y(Alignment::Center);
                    for (idx, (handle, _)) in shots.iter().enumerate() {
                        let is_sel = idx == active_idx;
                        let thumb_btn = button(
                            container(
                                image(handle.clone())
                                    .width(Length::Fixed(112.0))
                                    .height(Length::Fixed(66.0))
                                    .content_fit(ContentFit::Cover),
                            )
                            .padding(2)
                            .style(move |_| container::Style {
                                background: Some(iced::Background::Color(iced::Color::from_rgba(
                                    0.0, 0.0, 0.0, 0.35,
                                ))),
                                border: iced::Border {
                                    color: if is_sel {
                                        palette.primary.to_iced()
                                    } else {
                                        iced::Color::from_rgba(1.0, 1.0, 1.0, 0.10)
                                    },
                                    width: if is_sel { 2.0 } else { 1.0 },
                                    radius: iced::border::Radius::from(Radius::SM),
                                },
                                ..Default::default()
                            }),
                        )
                        .padding(0)
                        .style(|_, _| button::Style::default())
                        .on_press(Message::SelectScreenshot(idx));
                        thumbs = thumbs.push(thumb_btn);
                    }
                    gallery_col = gallery_col.push(thumbs);
                }

                Some(
                    container(gallery_col)
                        .padding(Spacing::MD)
                        .width(Length::Fill)
                        .style(move |_| card_style(palette))
                        .into(),
                )
            }
        } else if self.detail_loading && pkg.source == PackageSource::Flatpak {
            Some(
                    container(
                        row![
                            text(Icon::CAMERA)
                                .font(LUCIDE_FONT)
                                .size(16.0)
                                .color(palette.primary.to_iced()),
                            text("Flathub üzerinden ekran görüntüleri ve uygulama detayları yükleniyor...")
                                .size(Typography::BODY_SM)
                                .color(palette.text_muted.to_iced()),
                        ]
                        .spacing(Spacing::SM)
                        .align_y(Alignment::Center),
                    )
                    .padding(Spacing::MD)
                    .width(Length::Fill)
                    .style(move |_| card_style(palette))
                    .into(),
                )
        } else {
            None
        };

        // Security / Sandbox Röntgen Card
        let security_card = if let Some(ref perm) = pkg.permissions {
            let score_color = if perm.safety_score >= 85 {
                palette.success.to_iced()
            } else if perm.safety_score >= 65 {
                palette.warning.to_iced()
            } else {
                iced::Color::from_rgb(0.95, 0.35, 0.30)
            };

            let score_badge = container(
                text(format!("Güvenlik Skoru: {} / 100", perm.safety_score))
                    .size(11.0)
                    .color(score_color),
            )
            .padding([3, 8])
            .style(move |_| container::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgba(
                    1.0, 1.0, 1.0, 0.05,
                ))),
                border: iced::Border {
                    radius: iced::border::Radius::from(Radius::SM),
                    ..Default::default()
                },
                ..Default::default()
            });

            let mut fs_chips = row![].spacing(Spacing::XS);
            if perm.filesystems.is_empty() {
                fs_chips = fs_chips.push(
                    container(
                        text("Tamamen İzole (Dosya erişimi yok)")
                            .size(10.5)
                            .color(palette.success.to_iced()),
                    )
                    .padding([2, 6])
                    .style(|_| container::Style::default()),
                );
            } else {
                for fs in &perm.filesystems {
                    fs_chips = fs_chips.push(
                        container(
                            text(fs.clone())
                                .size(10.5)
                                .color(palette.text_primary.to_iced()),
                        )
                        .padding([2, 6])
                        .style(move |_| container::Style {
                            background: Some(iced::Background::Color(iced::Color::from_rgba(
                                1.0, 1.0, 1.0, 0.06,
                            ))),
                            border: iced::Border {
                                radius: iced::border::Radius::from(Radius::SM),
                                ..Default::default()
                            },
                            ..Default::default()
                        }),
                    );
                }
            }

            container(
                column![
                    row![
                        row![
                            text(Icon::SHIELD)
                                .font(LUCIDE_FONT)
                                .size(13.0)
                                .color(palette.primary.to_iced()),
                            text("CORTEN AI GÜVENLİK VE SANDBOX RÖNTGENİ")
                                .size(11.5)
                                .color(palette.primary.to_iced()),
                        ]
                        .spacing(Spacing::XS)
                        .align_y(Alignment::Center),
                        horizontal_space(),
                        score_badge,
                    ]
                    .align_y(Alignment::Center),
                    text(perm.safety_summary.clone())
                        .size(Typography::BODY_SM)
                        .color(palette.text_primary.to_iced()),
                    vertical_space().height(2),
                    row![
                        Self::perm_pill("Ağ Erişimi", perm.network, palette),
                        Self::perm_pill("Ses & Mikrofon", perm.pulseaudio, palette),
                        Self::perm_pill("Wayland", perm.wayland, palette),
                        Self::perm_pill("3D Hızlandırma", perm.dri, palette),
                    ]
                    .spacing(Spacing::XS),
                    vertical_space().height(2),
                    row![
                        text("Erişilen Dizinler:")
                            .size(10.5)
                            .color(palette.text_muted.to_iced()),
                        fs_chips,
                    ]
                    .spacing(Spacing::XS)
                    .align_y(Alignment::Center),
                ]
                .spacing(Spacing::XS),
            )
            .padding(Spacing::MD)
            .width(Length::Fill)
            .style(move |_| container::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgba(
                    1.0, 1.0, 1.0, 0.03,
                ))),
                border: iced::Border {
                    color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.08),
                    width: 1.0,
                    radius: iced::border::Radius::from(Radius::MD),
                },
                ..Default::default()
            })
        } else {
            container(
                row![
                    text(Icon::SHIELD).font(LUCIDE_FONT).size(Typography::TITLE_SM).color(palette.primary.to_iced()),
                    text("Bu paket yerel sistem depolarından (Arch Repo) sağlandığı için doğrudan sistem ayrıcalıklarıyla çalıştırılır.")
                        .size(11.5)
                        .color(palette.text_muted.to_iced()),
                ]
                .spacing(Spacing::SM)
                .align_y(Alignment::Center),
            )
            .padding(Spacing::MD)
            .width(Length::Fill)
            .style(move |_| container::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgba(1.0, 1.0, 1.0, 0.02))),
                border: iced::Border {
                    color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.05),
                    width: 1.0,
                    radius: iced::border::Radius::from(Radius::MD),
                },
                ..Default::default()
            })
        };

        // Description Section
        let desc_section = container(
            column![
                text("Açıklama")
                    .size(Typography::TITLE_SM)
                    .color(palette.text_primary.to_iced()),
                vertical_space().height(2),
                text(pkg.description.clone())
                    .size(Typography::BODY)
                    .color(palette.text_primary.to_iced()),
            ]
            .spacing(Spacing::XS),
        )
        .padding(Spacing::MD)
        .width(Length::Fill)
        .style(move |_| card_style(palette));

        let mut detail_col = column![app_header].spacing(Spacing::MD);
        if let Some(sc_el) = screenshots_section {
            detail_col = detail_col.push(sc_el);
        }
        detail_col = detail_col.push(desc_section).push(security_card);

        scrollable(detail_col).height(Length::Fill).into()
    }

    fn render_package_grid<'a>(
        &'a self,
        items: &[PackageItem],
        palette: Palette,
    ) -> Element<'a, Message> {
        let mut col = column![].spacing(Spacing::MD);
        let mut row_chunk = row![].spacing(Spacing::MD);
        let mut in_row = 0;

        for item in items {
            let card = self.render_package_card(item, palette);
            row_chunk = row_chunk.push(card);
            in_row += 1;

            if in_row == 2 {
                col = col.push(row_chunk);
                row_chunk = row![].spacing(Spacing::MD);
                in_row = 0;
            }
        }

        if in_row > 0 {
            col = col.push(row_chunk);
        }

        col.into()
    }

    fn render_package_card<'a>(
        &'a self,
        item: &PackageItem,
        palette: Palette,
    ) -> Element<'a, Message> {
        let item_clone = item.clone();
        let item_clone2 = item.clone();
        let (r, g, b) = item.source.badge_color();

        let source_pill = container(
            text(match item.source {
                PackageSource::Flatpak => "Flatpak",
                PackageSource::ArchRepo => "Arch Repo",
                PackageSource::Aur => "AUR",
                PackageSource::CortenCore => "Corten OTA",
            })
            .size(9.5)
            .color(iced::Color::from_rgb(r, g, b)),
        )
        .padding([2, 6])
        .style(move |_| container::Style {
            background: Some(iced::Background::Color(iced::Color::from_rgba(
                r, g, b, 0.15,
            ))),
            border: iced::Border {
                color: iced::Color::from_rgb(r, g, b),
                width: 1.0,
                radius: iced::border::Radius::from(Radius::SM),
            },
            ..Default::default()
        });

        let is_installed = item.state == PackageState::Installed;

        let action_btn = if is_installed {
            button(text("Kurulu").size(11.0).color(palette.success.to_iced()))
                .padding([4, 10])
                .style(move |_, _| button::Style {
                    background: Some(iced::Background::Color(iced::Color::from_rgba(
                        0.2, 0.8, 0.4, 0.15,
                    ))),
                    border: iced::Border {
                        color: iced::Color::from_rgba(0.2, 0.8, 0.4, 0.4),
                        width: 1.0,
                        radius: iced::border::Radius::from(Radius::SM),
                    },
                    ..Default::default()
                })
                .on_press(Message::OpenPackageDetail(item_clone))
        } else {
            button(text("Kur").size(11.0).color(palette.text_primary.to_iced()))
                .padding([4, 12])
                .style(move |_, _| button::Style {
                    background: Some(iced::Background::Color(palette.primary.to_iced())),
                    border: iced::Border {
                        radius: iced::border::Radius::from(Radius::SM),
                        ..Default::default()
                    },
                    ..Default::default()
                })
                .on_press(Message::InstallRequested(item_clone))
        };

        button(
            container(
                column![
                    row![
                        self.render_app_icon(&item.id, item.icon_char, 42.0, palette),
                        column![
                            row![
                                text(item.name.clone())
                                    .size(Typography::BODY)
                                    .color(palette.text_primary.to_iced()),
                                source_pill,
                            ]
                            .spacing(Spacing::XS)
                            .align_y(Alignment::Center),
                            text(format!("v{}", item.version))
                                .size(10.5)
                                .color(palette.text_muted.to_iced()),
                        ]
                        .spacing(2)
                        .width(Length::Fill),
                        action_btn,
                    ]
                    .spacing(Spacing::SM)
                    .align_y(Alignment::Center),
                    vertical_space().height(Spacing::XXS),
                    text(item.summary.clone())
                        .size(Typography::BODY_SM)
                        .color(palette.text_muted.to_iced()),
                ]
                .spacing(Spacing::XS),
            )
            .padding(Spacing::MD)
            .width(Length::FillPortion(1))
            .style(move |_| container::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgba(
                    1.0, 1.0, 1.0, 0.03,
                ))),
                border: iced::Border {
                    color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.06),
                    width: 1.0,
                    radius: iced::border::Radius::from(Radius::MD),
                },
                ..Default::default()
            }),
        )
        .padding(0)
        .style(|_, _| button::Style::default())
        .on_press(Message::OpenPackageDetail(item_clone2))
        .into()
    }

    // ─────────────────────────────────────────────────────────────────────────────
    // HELPERS
    // ─────────────────────────────────────────────────────────────────────────────

    fn render_app_icon<'a>(
        &'a self,
        app_id: &str,
        fallback_char: &'static str,
        size: f32,
        palette: Palette,
    ) -> Element<'a, Message> {
        if let Some(handle) = self.icon_handles.get(app_id) {
            container(
                image(handle.clone())
                    .width(Length::Fixed(size))
                    .height(Length::Fixed(size))
                    .content_fit(ContentFit::Contain),
            )
            .width(Length::Fixed(size))
            .height(Length::Fixed(size))
            .center_x(Length::Fixed(size))
            .center_y(Length::Fixed(size))
            .into()
        } else {
            let icon_sz = (size * 0.56).max(16.0);
            container(
                text(fallback_char)
                    .font(LUCIDE_FONT)
                    .size(icon_sz)
                    .color(palette.primary.to_iced()),
            )
            .width(Length::Fixed(size))
            .height(Length::Fixed(size))
            .center_x(Length::Fixed(size))
            .center_y(Length::Fixed(size))
            .style(move |_| container::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgba(
                    palette.primary.r,
                    palette.primary.g,
                    palette.primary.b,
                    0.12,
                ))),
                border: iced::Border {
                    color: iced::Color::from_rgba(
                        palette.primary.r,
                        palette.primary.g,
                        palette.primary.b,
                        0.28,
                    ),
                    width: 1.0,
                    radius: iced::border::Radius::from(Radius::MD),
                },
                ..Default::default()
            })
            .into()
        }
    }

    fn sidebar_nav_button<'a>(
        icon: char,
        label: &'static str,
        badge_count: Option<usize>,
        is_active: bool,
        palette: Palette,
        msg: Message,
    ) -> Element<'a, Message> {
        let badge: Element<'a, Message> = if let Some(count) = badge_count {
            container(
                text(count.to_string())
                    .size(9.5)
                    .color(palette.text_primary.to_iced()),
            )
            .padding([1, 6])
            .style(move |_| container::Style {
                background: Some(iced::Background::Color(if is_active {
                    palette.primary.to_iced()
                } else {
                    iced::Color::from_rgba(1.0, 1.0, 1.0, 0.10)
                })),
                border: iced::Border {
                    radius: iced::border::Radius::from(Radius::FULL),
                    ..Default::default()
                },
                ..Default::default()
            })
            .into()
        } else {
            horizontal_space().width(0).into()
        };

        button(
            container(
                row![
                    text(icon).font(LUCIDE_FONT).size(14.0).color(if is_active {
                        palette.primary.to_iced()
                    } else {
                        palette.text_muted.to_iced()
                    }),
                    text(label).size(Typography::BODY_SM).color(if is_active {
                        palette.text_primary.to_iced()
                    } else {
                        palette.text_muted.to_iced()
                    }),
                    horizontal_space(),
                    badge,
                ]
                .spacing(Spacing::SM)
                .align_y(Alignment::Center),
            )
            .padding([8, 12]),
        )
        .padding(0)
        .width(Length::Fill)
        .style(move |_, status| {
            let bg = if is_active {
                iced::Color::from_rgba(
                    palette.primary.r,
                    palette.primary.g,
                    palette.primary.b,
                    0.20,
                )
            } else {
                match status {
                    button::Status::Hovered => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.07),
                    _ => iced::Color::TRANSPARENT,
                }
            };
            button::Style {
                background: Some(iced::Background::Color(bg)),
                border: iced::Border {
                    color: if is_active {
                        palette.primary.to_iced()
                    } else {
                        iced::Color::TRANSPARENT
                    },
                    width: 1.0,
                    radius: iced::border::Radius::from(Radius::MD),
                },
                ..Default::default()
            }
        })
        .on_press(msg)
        .into()
    }

    fn category_nav_item<'a>(
        cat: ProductCategory,
        active_tab: &NavigationTab,
        palette: Palette,
    ) -> Element<'a, Message> {
        let is_active = matches!(active_tab, NavigationTab::Categories(c) if *c == cat);
        button(
            container(
                row![
                    text(cat.icon())
                        .font(LUCIDE_FONT)
                        .size(12.0)
                        .color(if is_active {
                            palette.primary.to_iced()
                        } else {
                            palette.text_muted.to_iced()
                        }),
                    text(cat.label()).size(11.5).color(if is_active {
                        palette.primary.to_iced()
                    } else {
                        palette.text_muted.to_iced()
                    }),
                ]
                .spacing(Spacing::XS)
                .align_y(Alignment::Center),
            )
            .padding([5, 8]),
        )
        .padding(0)
        .width(Length::Fill)
        .style(move |_, status| {
            let bg = if is_active {
                iced::Color::from_rgba(
                    palette.primary.r,
                    palette.primary.g,
                    palette.primary.b,
                    0.15,
                )
            } else {
                match status {
                    button::Status::Hovered => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.05),
                    _ => iced::Color::TRANSPARENT,
                }
            };
            button::Style {
                background: Some(iced::Background::Color(bg)),
                border: iced::Border {
                    radius: iced::border::Radius::from(Radius::SM),
                    ..Default::default()
                },
                ..Default::default()
            }
        })
        .on_press(Message::NavigateTo(NavigationTab::Categories(cat)))
        .into()
    }

    fn source_filter_chip<'a>(
        label: &'static str,
        filter: SourceFilter,
        current: SourceFilter,
        palette: Palette,
    ) -> Element<'a, Message> {
        let is_selected = filter == current;
        button(
            container(text(label).size(11.0).color(if is_selected {
                palette.text_primary.to_iced()
            } else {
                palette.text_muted.to_iced()
            }))
            .padding([4, 10]),
        )
        .padding(0)
        .style(move |_, status| {
            let bg = if is_selected {
                iced::Color::from_rgba(
                    palette.primary.r,
                    palette.primary.g,
                    palette.primary.b,
                    0.22,
                )
            } else {
                match status {
                    button::Status::Hovered => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.08),
                    _ => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.03),
                }
            };
            button::Style {
                background: Some(iced::Background::Color(bg)),
                border: iced::Border {
                    color: if is_selected {
                        palette.primary.to_iced()
                    } else {
                        iced::Color::from_rgba(1.0, 1.0, 1.0, 0.07)
                    },
                    width: 1.0,
                    radius: iced::border::Radius::from(Radius::FULL),
                },
                ..Default::default()
            }
        })
        .on_press(Message::SetSourceFilter(filter))
        .into()
    }

    fn perm_pill<'a>(title: &'static str, active: bool, palette: Palette) -> Element<'a, Message> {
        container(
            row![
                text(if active { "✓" } else { "—" })
                    .size(10.0)
                    .color(if active {
                        palette.primary.to_iced()
                    } else {
                        palette.text_muted.to_iced()
                    }),
                text(title).size(10.5).color(if active {
                    palette.text_primary.to_iced()
                } else {
                    palette.text_muted.to_iced()
                }),
            ]
            .spacing(4)
            .align_y(Alignment::Center),
        )
        .padding([4, 8])
        .style(move |_| container::Style {
            background: Some(iced::Background::Color(iced::Color::from_rgba(
                1.0, 1.0, 1.0, 0.04,
            ))),
            border: iced::Border {
                color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.06),
                width: 1.0,
                radius: iced::border::Radius::from(Radius::SM),
            },
            ..Default::default()
        })
        .into()
    }
}
