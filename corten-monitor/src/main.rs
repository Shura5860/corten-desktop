use std::collections::HashMap;
use std::fs;
use std::process::Command;
use std::time::Duration;

use corten_config::{AppearanceConfig, ConfigManager};
use corten_theme::iced_styles::{
    card_style, input_style, primary_button_style, secondary_button_style,
};
use corten_theme::{
    Icon, Palette, PatinaVariant, Radius, Spacing, ThemeMode, Typography, LUCIDE_FONT,
    LUCIDE_FONT_BYTES, NOTO_SANS, NOTO_SANS_BYTES,
};
use iced::widget::{
    button, column, container, horizontal_space, progress_bar, row, scrollable, text, text_input,
};
use iced::{border, event, keyboard, time, Alignment, Border, Color, Element, Length, Subscription, Task};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcSortColumn {
    Cpu,
    Memory,
    Pid,
    Name,
}

#[derive(Debug, Clone)]
pub struct ProcessEntry {
    pub pid: u32,
    pub name: String,
    pub state: char,
    pub cpu_percent: f32,
    pub rss_bytes: u64,
    pub threads: u32,
}

#[derive(Debug, Clone, Default)]
pub struct SystemSnapshot {
    pub cpu_percent: f32,
    pub mem_used_bytes: u64,
    pub mem_total_bytes: u64,
    pub swap_used_bytes: u64,
    pub swap_total_bytes: u64,
    pub load_avg: (f32, f32, f32),
    pub uptime_secs: u64,
    pub net_rx_speed: u64,
    pub net_tx_speed: u64,
    pub disk_read_speed: u64,
    pub disk_write_speed: u64,
    pub processes: Vec<ProcessEntry>,
}

#[derive(Debug, Clone, Default)]
pub struct ProcSampler {
    prev_total_ticks: u64,
    prev_idle_ticks: u64,
    prev_proc_ticks: HashMap<u32, u64>,
    prev_net_rx: u64,
    prev_net_tx: u64,
    prev_disk_read: u64,
    prev_disk_write: u64,
    last_sample_instant: Option<std::time::Instant>,
}

impl ProcSampler {
    pub fn sample(&mut self) -> SystemSnapshot {
        let (total_ticks, idle_ticks) = read_cpu_ticks().unwrap_or((0, 0));
        let delta_total = total_ticks.saturating_sub(self.prev_total_ticks);
        let delta_idle = idle_ticks.saturating_sub(self.prev_idle_ticks);

        let cpu_percent = if self.prev_total_ticks > 0 && delta_total > 0 {
            let busy = delta_total.saturating_sub(delta_idle);
            ((busy as f32 / delta_total as f32) * 100.0).clamp(0.0, 100.0)
        } else {
            0.0
        };

        self.prev_total_ticks = total_ticks;
        self.prev_idle_ticks = idle_ticks;

        let (mem_total_bytes, mem_used_bytes, swap_total_bytes, swap_used_bytes) = read_meminfo();
        let load_avg = read_loadavg();
        let uptime_secs = read_uptime();
        let num_cpus = std::thread::available_parallelism()
            .map(|n| n.get() as f32)
            .unwrap_or(1.0);

        let mut next_proc_ticks = HashMap::new();
        let mut processes = Vec::new();
        let page_size = 4096_u64;

        if let Ok(entries) = fs::read_dir("/proc") {
            for entry in entries.flatten() {
                let file_name = entry.file_name();
                let Some(pid_str) = file_name.to_str() else {
                    continue;
                };
                let Ok(pid) = pid_str.parse::<u32>() else {
                    continue;
                };

                let stat_path = format!("/proc/{}/stat", pid_str);
                let Ok(stat_raw) = fs::read_to_string(&stat_path) else {
                    continue;
                };
                let Some((name, state, proc_ticks, rss_pages, threads)) =
                    parse_proc_stat(&stat_raw)
                else {
                    continue;
                };

                next_proc_ticks.insert(pid, proc_ticks);
                let prev_ticks = self.prev_proc_ticks.get(&pid).copied().unwrap_or(proc_ticks);
                let proc_delta = proc_ticks.saturating_sub(prev_ticks);
                let proc_cpu = if delta_total > 0 {
                    ((proc_delta as f32 / delta_total as f32) * 100.0 * num_cpus)
                        .clamp(0.0, 100.0 * num_cpus)
                } else {
                    0.0
                };

                processes.push(ProcessEntry {
                    pid,
                    name,
                    state,
                    cpu_percent: proc_cpu,
                    rss_bytes: rss_pages.saturating_mul(page_size),
                    threads,
                });
            }
        }

        self.prev_proc_ticks = next_proc_ticks;

        let now = std::time::Instant::now();
        let elapsed_secs = if let Some(last) = self.last_sample_instant {
            now.duration_since(last).as_secs_f64().max(0.1)
        } else {
            1.5
        };
        self.last_sample_instant = Some(now);

        let (cur_net_rx, cur_net_tx) = read_net_bytes();
        let (cur_disk_read, cur_disk_write) = read_disk_bytes();

        let net_rx_speed = if self.prev_net_rx > 0 {
            (cur_net_rx.saturating_sub(self.prev_net_rx) as f64 / elapsed_secs) as u64
        } else {
            0
        };
        let net_tx_speed = if self.prev_net_tx > 0 {
            (cur_net_tx.saturating_sub(self.prev_net_tx) as f64 / elapsed_secs) as u64
        } else {
            0
        };
        let disk_read_speed = if self.prev_disk_read > 0 {
            (cur_disk_read.saturating_sub(self.prev_disk_read) as f64 / elapsed_secs) as u64
        } else {
            0
        };
        let disk_write_speed = if self.prev_disk_write > 0 {
            (cur_disk_write.saturating_sub(self.prev_disk_write) as f64 / elapsed_secs) as u64
        } else {
            0
        };

        self.prev_net_rx = cur_net_rx;
        self.prev_net_tx = cur_net_tx;
        self.prev_disk_read = cur_disk_read;
        self.prev_disk_write = cur_disk_write;

        SystemSnapshot {
            cpu_percent,
            mem_used_bytes,
            mem_total_bytes,
            swap_used_bytes,
            swap_total_bytes,
            load_avg,
            uptime_secs,
            net_rx_speed,
            net_tx_speed,
            disk_read_speed,
            disk_write_speed,
            processes,
        }
    }
}

pub fn parse_proc_stat(stat_raw: &str) -> Option<(String, char, u64, u64, u32)> {
    let open_paren = stat_raw.find('(')?;
    let close_paren = stat_raw.rfind(')')?;
    if close_paren <= open_paren {
        return None;
    }
    let name = stat_raw[open_paren + 1..close_paren].to_string();
    let rest = stat_raw.get(close_paren + 2..)?;

    let mut iter = rest.split_whitespace();
    let state = iter.next()?.chars().next().unwrap_or('S');

    let mut utime: u64 = 0;
    let mut stime: u64 = 0;
    let mut threads: u32 = 1;
    let mut rss_pages: u64 = 0;
    let mut idx = 1;

    for field in iter {
        match idx {
            11 => utime = field.parse().ok()?,
            12 => stime = field.parse().ok()?,
            17 => threads = field.parse().unwrap_or(1),
            21 => {
                rss_pages = field.parse().unwrap_or(0);
                break;
            }
            _ => {}
        }
        idx += 1;
    }

    if idx < 21 {
        return None;
    }

    Some((name, state, utime.saturating_add(stime), rss_pages, threads))
}

fn read_cpu_ticks() -> Option<(u64, u64)> {
    let content = fs::read_to_string("/proc/stat").ok()?;
    let first_line = content.lines().next()?;
    let mut parts = first_line.split_whitespace();
    if parts.next()? != "cpu" {
        return None;
    }
    let vals: Vec<u64> = parts.filter_map(|p| p.parse::<u64>().ok()).collect();
    if vals.len() < 4 {
        return None;
    }
    // vals: [user, nice, system, idle, iowait, irq, softirq, steal, ...]
    // guest and guest_nice (indexes 8, 9) are already accounted for in user and nice by the Linux kernel.
    let total: u64 = if vals.len() >= 8 {
        vals[..8].iter().sum()
    } else {
        vals.iter().sum()
    };
    let idle = vals[3].saturating_add(vals.get(4).copied().unwrap_or(0));
    Some((total, idle))
}

fn read_meminfo() -> (u64, u64, u64, u64) {
    let Ok(content) = fs::read_to_string("/proc/meminfo") else {
        return (1, 0, 0, 0);
    };
    let mut mem_total_kb = 0_u64;
    let mut mem_avail_kb = 0_u64;
    let mut swap_total_kb = 0_u64;
    let mut swap_free_kb = 0_u64;

    for line in content.lines() {
        let mut parts = line.split_whitespace();
        let Some(key) = parts.next() else {
            continue;
        };
        let Some(val_str) = parts.next() else {
            continue;
        };
        let Ok(val) = val_str.parse::<u64>() else {
            continue;
        };
        match key {
            "MemTotal:" => mem_total_kb = val,
            "MemAvailable:" => mem_avail_kb = val,
            "SwapTotal:" => swap_total_kb = val,
            "SwapFree:" => swap_free_kb = val,
            _ => {}
        }
    }

    let mem_total = mem_total_kb.saturating_mul(1024);
    let mem_used = mem_total_kb.saturating_sub(mem_avail_kb).saturating_mul(1024);
    let swap_total = swap_total_kb.saturating_mul(1024);
    let swap_used = swap_total_kb.saturating_sub(swap_free_kb).saturating_mul(1024);
    (mem_total, mem_used, swap_total, swap_used)
}

fn read_loadavg() -> (f32, f32, f32) {
    let Ok(content) = fs::read_to_string("/proc/loadavg") else {
        return (0.0, 0.0, 0.0);
    };
    let mut parts = content.split_whitespace();
    let l1 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
    let l5 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
    let l15 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
    (l1, l5, l15)
}

fn read_uptime() -> u64 {
    let Ok(content) = fs::read_to_string("/proc/uptime") else {
        return 0;
    };
    content
        .split_whitespace()
        .next()
        .and_then(|s| s.parse::<f64>().ok())
        .map(|v| v as u64)
        .unwrap_or(0)
}

pub fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.2} GB", b / GB)
    } else if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.0} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

pub fn format_uptime(secs: u64) -> String {
    let hours = secs / 3600;
    let mins = (secs % 3600) / 60;
    if hours > 0 {
        format!("{hours}sa {mins}dk")
    } else {
        format!("{mins}dk {}sn", secs % 60)
    }
}

fn read_net_bytes() -> (u64, u64) {
    let Ok(content) = fs::read_to_string("/proc/net/dev") else {
        return (0, 0);
    };
    let mut total_rx = 0;
    let mut total_tx = 0;
    for line in content.lines().skip(2) {
        let Some((iface, rest)) = line.split_once(':') else {
            continue;
        };
        let iface = iface.trim();
        if iface == "lo" {
            continue;
        }
        let mut parts = rest.split_whitespace();
        let rx: u64 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        let tx: u64 = parts.nth(7).and_then(|s| s.parse().ok()).unwrap_or(0);
        total_rx += rx;
        total_tx += tx;
    }
    (total_rx, total_tx)
}

fn read_disk_bytes() -> (u64, u64) {
    let Ok(content) = fs::read_to_string("/proc/diskstats") else {
        return (0, 0);
    };
    let mut total_read = 0;
    let mut total_write = 0;
    for line in content.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 14 {
            continue;
        }
        let dev_name = parts[2];
        if dev_name.starts_with("loop")
            || dev_name.starts_with("ram")
            || dev_name.starts_with("zram")
        {
            continue;
        }
        let sectors_read: u64 = parts[5].parse().unwrap_or(0);
        let sectors_written: u64 = parts[9].parse().unwrap_or(0);
        total_read += sectors_read * 512;
        total_write += sectors_written * 512;
    }
    (total_read, total_write)
}

#[derive(Debug, Clone)]
pub struct ServiceEntry {
    pub unit: String,
    pub load: String,
    pub active: String,
    pub sub: String,
    pub description: String,
}

pub fn list_systemd_services() -> Vec<ServiceEntry> {
    let Ok(output) = Command::new("systemctl")
        .args(["list-units", "--type=service", "--no-legend", "--no-pager"])
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut services = Vec::new();
    for line in text.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 5 {
            let unit = parts[0].trim_start_matches('●').trim().to_string();
            let load = parts[1].to_string();
            let active = parts[2].to_string();
            let sub = parts[3].to_string();
            let description = parts[4..].join(" ");
            services.push(ServiceEntry {
                unit,
                load,
                active,
                sub,
                description,
            });
        }
    }
    services
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MonitorTab {
    #[default]
    Processes,
    Services,
}

#[derive(Debug, Clone)]
pub enum Message {
    Tick,
    SearchChanged(String),
    SortBy(ProcSortColumn),
    SelectPid(u32),
    SendSignal(u32, &'static str),
    SignalResult { pid: u32, sig: &'static str, success: bool },
    SwitchTab(MonitorTab),
    RestartService(String),
    StopService(String),
    ServiceActionResult { unit: String, action: String, success: bool },
    ClearSelectionOrSearch,
    KillSelected(bool),
}

pub fn main() -> iced::Result {
    tracing_subscriber::fmt::init();

    iced::application(
        "Corten Monitor — Sistem Görev Yöneticisi",
        CortenMonitorApp::update,
        CortenMonitorApp::view,
    )
    .window(iced::window::Settings {
        size: iced::Size::new(980.0, 680.0),
        min_size: Some(iced::Size::new(760.0, 500.0)),
        resizable: true,
        #[cfg(target_os = "linux")]
        platform_specific: iced::window::settings::PlatformSpecific {
            application_id: "corten-monitor".into(),
            ..Default::default()
        },
        ..Default::default()
    })
    .subscription(CortenMonitorApp::subscription)
    .font(LUCIDE_FONT_BYTES)
    .font(NOTO_SANS_BYTES)
    .default_font(NOTO_SANS)
    .run()
}

pub struct CortenMonitorApp {
    sampler: ProcSampler,
    snapshot: SystemSnapshot,
    search_query: String,
    sort_col: ProcSortColumn,
    sort_desc: bool,
    selected_pid: Option<u32>,
    status_message: String,
    theme_mode: ThemeMode,
    active_patina: PatinaVariant,
    current_tab: MonitorTab,
    services: Vec<ServiceEntry>,
}

impl Default for CortenMonitorApp {
    fn default() -> Self {
        let mut sampler = ProcSampler::default();
        // Prime first delta sample
        let _ = sampler.sample();
        let snapshot = sampler.sample();
        let services = list_systemd_services();
        let active_patina = ConfigManager::<AppearanceConfig>::load_or_default("appearance")
            .map(|c| PatinaVariant::from_code(&c.get().patina))
            .unwrap_or(PatinaVariant::Rust);

        Self {
            sampler,
            snapshot,
            search_query: String::new(),
            sort_col: ProcSortColumn::Memory,
            sort_desc: true,
            selected_pid: None,
            status_message: "Sistem kaynakları canlı izleniyor (1.5sn yenileme)".to_string(),
            theme_mode: ThemeMode::Dark,
            active_patina,
            current_tab: MonitorTab::Processes,
            services,
        }
    }
}

impl CortenMonitorApp {
    pub fn subscription(&self) -> Subscription<Message> {
        let timer_sub = time::every(Duration::from_millis(1500)).map(|_| Message::Tick);
        let key_sub = event::listen_with(|event, _status, _window| match event {
            iced::Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) => match key {
                keyboard::Key::Named(keyboard::key::Named::F5) => Some(Message::Tick),
                keyboard::Key::Named(keyboard::key::Named::Escape) => {
                    Some(Message::ClearSelectionOrSearch)
                }
                keyboard::Key::Named(keyboard::key::Named::Delete) => {
                    Some(Message::KillSelected(modifiers.shift()))
                }
                keyboard::Key::Character(ref c) if (c == "r" || c == "R") && modifiers.control() => {
                    Some(Message::Tick)
                }
                keyboard::Key::Character(ref c) if c == "1" && modifiers.alt() => {
                    Some(Message::SwitchTab(MonitorTab::Processes))
                }
                keyboard::Key::Character(ref c) if c == "2" && modifiers.alt() => {
                    Some(Message::SwitchTab(MonitorTab::Services))
                }
                _ => None,
            },
            _ => None,
        });

        Subscription::batch(vec![timer_sub, key_sub])
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Tick => {
                self.snapshot = self.sampler.sample();
                if self.current_tab == MonitorTab::Services {
                    self.services = list_systemd_services();
                }
                if let Ok(cfg) = ConfigManager::<AppearanceConfig>::load_or_default("appearance") {
                    let new_patina = PatinaVariant::from_code(&cfg.get().patina);
                    if self.active_patina != new_patina {
                        self.active_patina = new_patina;
                    }
                }
            }
            Message::SwitchTab(tab) => {
                self.current_tab = tab;
                if tab == MonitorTab::Services {
                    self.services = list_systemd_services();
                }
            }
            Message::SearchChanged(q) => {
                self.search_query = q;
            }
            Message::ClearSelectionOrSearch => {
                if !self.search_query.is_empty() {
                    self.search_query.clear();
                } else {
                    self.selected_pid = None;
                }
            }
            Message::SortBy(col) => {
                if self.sort_col == col {
                    self.sort_desc = !self.sort_desc;
                } else {
                    self.sort_col = col;
                    self.sort_desc = col != ProcSortColumn::Name;
                }
            }
            Message::SelectPid(pid) => {
                self.selected_pid = Some(pid);
            }
            Message::KillSelected(force) => {
                if let Some(pid) = self.selected_pid {
                    let sig = if force { "KILL" } else { "TERM" };
                    return Task::perform(
                        async move {
                            let flag = format!("-{sig}");
                            let res = tokio::process::Command::new("kill")
                                .arg(&flag)
                                .arg(pid.to_string())
                                .status()
                                .await;
                            (pid, sig, res.map(|s| s.success()).unwrap_or(false))
                        },
                        |(pid, sig, success)| Message::SignalResult { pid, sig, success },
                    );
                }
            }
            Message::SendSignal(pid, sig) => {
                return Task::perform(
                    async move {
                        let flag = format!("-{sig}");
                        let res = tokio::process::Command::new("kill")
                            .arg(&flag)
                            .arg(pid.to_string())
                            .status()
                            .await;
                        (pid, sig, res.map(|s| s.success()).unwrap_or(false))
                    },
                    |(pid, sig, success)| Message::SignalResult { pid, sig, success },
                );
            }
            Message::SignalResult { pid, sig, success } => {
                if success {
                    self.status_message = format!("PID {pid} sürecine SIG{sig} sinyali gönderildi.");
                    self.snapshot = self.sampler.sample();
                } else {
                    self.status_message = format!("PID {pid} sonlandırılamadı (Erişim engellendi veya süreç sonlandı).");
                }
            }
            Message::RestartService(unit) => {
                let unit_clone = unit.clone();
                return Task::perform(
                    async move {
                        let res = tokio::process::Command::new("systemctl")
                            .arg("restart")
                            .arg(&unit_clone)
                            .status()
                            .await;
                        (unit_clone, "yeniden başlatıldı".to_string(), res.map(|s| s.success()).unwrap_or(false))
                    },
                    |(unit, action, success)| Message::ServiceActionResult { unit, action, success },
                );
            }
            Message::StopService(unit) => {
                let unit_clone = unit.clone();
                return Task::perform(
                    async move {
                        let res = tokio::process::Command::new("systemctl")
                            .arg("stop")
                            .arg(&unit_clone)
                            .status()
                            .await;
                        (unit_clone, "durduruldu".to_string(), res.map(|s| s.success()).unwrap_or(false))
                    },
                    |(unit, action, success)| Message::ServiceActionResult { unit, action, success },
                );
            }
            Message::ServiceActionResult { unit, action, success } => {
                if success {
                    self.status_message = format!("Servis {action}: {unit}");
                } else {
                    self.status_message = format!("Servis işlemi başarısız: {unit} ({action})");
                }
                self.services = list_systemd_services();
            }
        }
        Task::none()
    }

    fn filtered_processes(&self) -> Vec<ProcessEntry> {
        let query = self.search_query.trim().to_ascii_lowercase();
        let mut list: Vec<ProcessEntry> = self
            .snapshot
            .processes
            .iter()
            .filter(|p| {
                query.is_empty()
                    || p.name.to_ascii_lowercase().contains(&query)
                    || p.pid.to_string().contains(&query)
            })
            .cloned()
            .collect();

        list.sort_by(|a, b| {
            let ord = match self.sort_col {
                ProcSortColumn::Cpu => a
                    .cpu_percent
                    .partial_cmp(&b.cpu_percent)
                    .unwrap_or(std::cmp::Ordering::Equal),
                ProcSortColumn::Memory => a.rss_bytes.cmp(&b.rss_bytes),
                ProcSortColumn::Pid => a.pid.cmp(&b.pid),
                ProcSortColumn::Name => a
                    .name
                    .to_ascii_lowercase()
                    .cmp(&b.name.to_ascii_lowercase()),
            };
            if self.sort_desc {
                ord.reverse()
            } else {
                ord
            }
        });
        list
    }

    pub fn view(&self) -> Element<'_, Message> {
        let palette = Palette::for_mode(self.theme_mode).with_patina(self.active_patina);

        let mem_pct = if self.snapshot.mem_total_bytes > 0 {
            (self.snapshot.mem_used_bytes as f32 / self.snapshot.mem_total_bytes as f32) * 100.0
        } else {
            0.0
        };

        let gauges_row = row![
            self.view_gauge_card(
                "CPU Kullanımı",
                format!("{:.1}%", self.snapshot.cpu_percent),
                format!(
                    "Yük: {:.2} / {:.2} / {:.2}",
                    self.snapshot.load_avg.0, self.snapshot.load_avg.1, self.snapshot.load_avg.2
                ),
                self.snapshot.cpu_percent,
                palette,
            ),
            self.view_gauge_card(
                "Bellek (RAM)",
                format!("{:.1}%", mem_pct),
                format!(
                    "{} / {}",
                    format_bytes(self.snapshot.mem_used_bytes),
                    format_bytes(self.snapshot.mem_total_bytes)
                ),
                mem_pct,
                palette,
            ),
            self.view_gauge_card(
                "Ağ Trafiği",
                format!("↓ {}/sn", format_bytes(self.snapshot.net_rx_speed)),
                format!("↑ {}/sn (Gönderim)", format_bytes(self.snapshot.net_tx_speed)),
                ((self.snapshot.net_rx_speed as f32 / 10_000_000.0) * 100.0).clamp(0.0, 100.0),
                palette,
            ),
            self.view_gauge_card(
                "Disk I/O",
                format!("R: {}/sn", format_bytes(self.snapshot.disk_read_speed)),
                format!(
                    "W: {}/sn • Açık: {}",
                    format_bytes(self.snapshot.disk_write_speed),
                    format_uptime(self.snapshot.uptime_secs)
                ),
                ((self.snapshot.disk_read_speed as f32 / 50_000_000.0) * 100.0).clamp(0.0, 100.0),
                palette,
            ),
        ]
        .spacing(Spacing::MD);

        let tab_btn = |tab: MonitorTab, label: &'static str| {
            let is_active = self.current_tab == tab;
            button(
                text(label)
                    .size(Typography::BODY_SM)
                    .color(if is_active {
                        palette.primary.to_iced()
                    } else {
                        palette.text_muted.to_iced()
                    }),
            )
            .padding([4, 12])
            .style(move |_, status| {
                let mut st = secondary_button_style(palette, status);
                if is_active {
                    st.border = Border {
                        color: palette.primary.to_iced(),
                        width: 1.0,
                        radius: border::Radius::from(Radius::SM),
                    };
                }
                st
            })
            .on_press(Message::SwitchTab(tab))
        };

        let processes = self.filtered_processes();
        let selected_proc = self
            .selected_pid
            .and_then(|pid| processes.iter().find(|p| p.pid == pid).cloned());

        let mut toolbar = row![
            tab_btn(MonitorTab::Processes, "İşlemler"),
            tab_btn(MonitorTab::Services, "Sistem Servisleri"),
            container(
                row![
                    text(Icon::SEARCH)
                        .font(LUCIDE_FONT)
                        .size(14)
                        .color(palette.text_muted.to_iced()),
                    text_input("Süreç, PID veya servis ara...", &self.search_query)
                        .on_input(Message::SearchChanged)
                        .padding([6, 8])
                        .size(Typography::BODY_SM)
                        .width(Length::Fixed(240.0)),
                ]
                .spacing(Spacing::XS)
                .align_y(Alignment::Center),
            )
            .padding([2, 8])
            .style(move |_| input_style(palette)),
            text(if self.current_tab == MonitorTab::Processes {
                format!("{} süreç", processes.len())
            } else {
                format!("{} servis", self.services.len())
            })
            .size(Typography::BODY_SM)
            .color(palette.text_muted.to_iced()),
            horizontal_space(),
        ]
        .spacing(Spacing::SM)
        .align_y(Alignment::Center);

        if self.current_tab == MonitorTab::Processes {
            if let Some(proc_info) = selected_proc {
                toolbar = toolbar
                    .push(
                        text(format!("Seçili: {} (PID {})", proc_info.name, proc_info.pid))
                            .size(Typography::BODY_SM)
                            .color(palette.primary.to_iced()),
                    )
                    .push(
                        button(text("Sonlandır (SIGTERM)").size(Typography::CAPTION))
                            .padding([6, 12])
                            .on_press(Message::SendSignal(proc_info.pid, "TERM"))
                            .style(move |_, status| secondary_button_style(palette, status)),
                    )
                    .push(
                        button(text("Zorla Kapat (SIGKILL)").size(Typography::CAPTION))
                            .padding([6, 12])
                            .on_press(Message::SendSignal(proc_info.pid, "KILL"))
                            .style(move |_, status| {
                                let mut s = primary_button_style(palette, status);
                                s.background = Some(palette.danger.to_iced().into());
                                s
                            }),
                    );
            }
        }

        let table_card = if self.current_tab == MonitorTab::Processes {
            let header = container(
                row![
                    Self::sort_button(
                        "PID",
                        ProcSortColumn::Pid,
                        self.sort_col,
                        self.sort_desc,
                        Length::Fixed(80.0),
                        palette
                    ),
                    Self::sort_button(
                        "Süreç Adı",
                        ProcSortColumn::Name,
                        self.sort_col,
                        self.sort_desc,
                        Length::FillPortion(4),
                        palette
                    ),
                    Self::sort_button(
                        "CPU %",
                        ProcSortColumn::Cpu,
                        self.sort_col,
                        self.sort_desc,
                        Length::Fixed(100.0),
                        palette
                    ),
                    Self::sort_button(
                        "Bellek (RSS)",
                        ProcSortColumn::Memory,
                        self.sort_col,
                        self.sort_desc,
                        Length::Fixed(130.0),
                        palette
                    ),
                    text("Durum / İş Parçacığı")
                        .size(Typography::CAPTION)
                        .color(palette.text_muted.to_iced())
                        .width(Length::Fixed(130.0)),
                ]
                .spacing(Spacing::SM)
                .align_y(Alignment::Center),
            )
            .padding([Spacing::XS, Spacing::SM])
            .style(move |_| container::Style {
                background: Some(palette.surface_hover.to_iced().into()),
                border: Border {
                    radius: border::Radius::from(Radius::SM),
                    width: 1.0,
                    color: palette.border.to_iced(),
                },
                ..Default::default()
            });

            let mut rows_col = column![].spacing(2);
            for proc_item in processes.into_iter().take(200) {
                let is_selected = self.selected_pid == Some(proc_item.pid);
                let pid = proc_item.pid;
                let row_btn = button(
                    row![
                        text(proc_item.pid.to_string())
                            .size(Typography::BODY_SM)
                            .color(palette.text_muted.to_iced())
                            .width(Length::Fixed(80.0)),
                        text(proc_item.name)
                            .size(Typography::BODY_SM)
                            .color(palette.text_primary.to_iced())
                            .width(Length::FillPortion(4)),
                        text(format!("{:.1}%", proc_item.cpu_percent))
                            .size(Typography::BODY_SM)
                            .color(if proc_item.cpu_percent > 25.0 {
                                palette.primary.to_iced()
                            } else {
                                palette.text_muted.to_iced()
                            })
                            .width(Length::Fixed(100.0)),
                        text(format_bytes(proc_item.rss_bytes))
                            .size(Typography::BODY_SM)
                            .color(palette.text_muted.to_iced())
                            .width(Length::Fixed(130.0)),
                        text(format!("{} • {} th", proc_item.state, proc_item.threads))
                            .size(Typography::CAPTION)
                            .color(palette.text_muted.to_iced())
                            .width(Length::Fixed(130.0)),
                    ]
                    .spacing(Spacing::SM)
                    .align_y(Alignment::Center),
                )
                .width(Length::Fill)
                .padding([6, 10])
                .on_press(Message::SelectPid(pid))
                .style(move |_, status| {
                    let bg = if is_selected {
                        Color {
                            a: 0.22,
                            ..palette.primary.to_iced()
                        }
                    } else if matches!(status, button::Status::Hovered) {
                        palette.surface_hover.to_iced()
                    } else {
                        Color::TRANSPARENT
                    };
                    button::Style {
                        background: Some(bg.into()),
                        text_color: palette.text_primary.to_iced(),
                        border: Border {
                            radius: border::Radius::from(Radius::SM),
                            width: if is_selected { 1.0 } else { 0.0 },
                            color: if is_selected {
                                palette.primary.to_iced()
                            } else {
                                Color::TRANSPARENT
                            },
                        },
                        ..Default::default()
                    }
                });
                rows_col = rows_col.push(row_btn);
            }

            container(
                column![
                    header,
                    scrollable(rows_col).height(Length::Fill),
                ]
                .spacing(Spacing::XS),
            )
            .padding(Spacing::SM)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(move |_| card_style(palette))
        } else {
            // Services View
            let header = container(
                row![
                    text("Servis Adı (Birim)")
                        .size(Typography::CAPTION)
                        .color(palette.primary.to_iced())
                        .width(Length::FillPortion(3)),
                    text("Durum")
                        .size(Typography::CAPTION)
                        .color(palette.text_muted.to_iced())
                        .width(Length::Fixed(120.0)),
                    text("Açıklama")
                        .size(Typography::CAPTION)
                        .color(palette.text_muted.to_iced())
                        .width(Length::FillPortion(4)),
                    text("İşlemler")
                        .size(Typography::CAPTION)
                        .color(palette.text_muted.to_iced())
                        .width(Length::Fixed(160.0)),
                ]
                .spacing(Spacing::SM)
                .align_y(Alignment::Center),
            )
            .padding([Spacing::XS, Spacing::SM])
            .style(move |_| container::Style {
                background: Some(palette.surface_hover.to_iced().into()),
                border: Border {
                    radius: border::Radius::from(Radius::SM),
                    width: 1.0,
                    color: palette.border.to_iced(),
                },
                ..Default::default()
            });

            let q = self.search_query.trim().to_lowercase();
            let mut s_rows = column![].spacing(4);
            for s in &self.services {
                if !q.is_empty()
                    && !s.unit.to_lowercase().contains(&q)
                    && !s.description.to_lowercase().contains(&q)
                {
                    continue;
                }
                let unit = s.unit.clone();
                let is_running = s.active == "active" || s.sub == "running";
                let s_row = container(
                    row![
                        text(&s.unit)
                            .size(Typography::BODY_SM)
                            .color(palette.text_primary.to_iced())
                            .width(Length::FillPortion(3)),
                        text(format!("{} ({})", s.active, s.sub))
                            .size(Typography::CAPTION)
                            .color(if is_running {
                                palette.success.to_iced()
                            } else {
                                palette.danger.to_iced()
                            })
                            .width(Length::Fixed(120.0)),
                        text(&s.description)
                            .size(Typography::CAPTION)
                            .color(palette.text_muted.to_iced())
                            .width(Length::FillPortion(4)),
                        row![
                            button(text("Yeniden Başlat").size(10.0))
                                .padding([3, 6])
                                .style(move |_, st| secondary_button_style(palette, st))
                                .on_press(Message::RestartService(unit.clone())),
                            button(text("Durdur").size(10.0))
                                .padding([3, 6])
                                .style(move |_, st| {
                                    let mut b = secondary_button_style(palette, st);
                                    b.text_color = palette.danger.to_iced();
                                    b
                                })
                                .on_press(Message::StopService(unit.clone())),
                        ]
                        .spacing(4)
                        .width(Length::Fixed(160.0)),
                    ]
                    .spacing(Spacing::SM)
                    .align_y(Alignment::Center),
                )
                .padding([4, 8]);

                s_rows = s_rows.push(s_row);
            }

            container(
                column![
                    header,
                    scrollable(s_rows).height(Length::Fill),
                ]
                .spacing(Spacing::XS),
            )
            .padding(Spacing::SM)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(move |_| card_style(palette))
        };

        let status_bar = container(
            row![
                text(&self.status_message)
                    .size(Typography::CAPTION)
                    .color(palette.text_muted.to_iced()),
                horizontal_space(),
                text("Corten Sistem İzleyici • /proc doğrudan okuma")
                    .size(Typography::CAPTION)
                    .color(palette.text_muted.to_iced()),
            ]
            .align_y(Alignment::Center),
        )
        .padding([Spacing::XS, Spacing::SM])
        .width(Length::Fill)
        .style(move |_| card_style(palette));

        container(
            column![gauges_row, toolbar, table_card, status_bar]
                .spacing(Spacing::MD)
                .height(Length::Fill),
        )
        .padding(Spacing::MD)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(move |_| container::Style {
            background: Some(palette.background.to_iced().into()),
            text_color: Some(palette.text_primary.to_iced()),
            ..Default::default()
        })
        .into()
    }

    fn view_gauge_card<'a>(
        &self,
        title: &'a str,
        big_value: String,
        subtitle: String,
        pct: f32,
        palette: Palette,
    ) -> Element<'a, Message> {
        let clamped = pct.clamp(0.0, 100.0);
        container(
            column![
                row![
                    text(title)
                        .size(Typography::BODY_SM)
                        .color(palette.text_muted.to_iced()),
                    horizontal_space(),
                    text(big_value)
                        .size(Typography::TITLE)
                        .color(palette.primary.to_iced()),
                ]
                .align_y(Alignment::Center),
                progress_bar(0.0..=100.0, clamped).height(8),
                text(subtitle)
                    .size(Typography::CAPTION)
                    .color(palette.text_muted.to_iced()),
            ]
            .spacing(Spacing::XS),
        )
        .padding(Spacing::MD)
        .width(Length::FillPortion(1))
        .style(move |_| card_style(palette))
        .into()
    }

    fn sort_button<'a>(
        label: &'a str,
        col: ProcSortColumn,
        active_col: ProcSortColumn,
        desc: bool,
        width: Length,
        palette: Palette,
    ) -> Element<'a, Message> {
        let is_active = col == active_col;
        let arrow = if !is_active {
            ""
        } else if desc {
            " ↓"
        } else {
            " ↑"
        };
        button(
            text(format!("{label}{arrow}"))
                .size(Typography::CAPTION)
                .color(if is_active {
                    palette.primary.to_iced()
                } else {
                    palette.text_muted.to_iced()
                }),
        )
        .width(width)
        .padding([4, 6])
        .on_press(Message::SortBy(col))
        .style(move |_, status| {
            if is_active {
                let mut s = primary_button_style(palette, status);
                s.background = Some(palette.surface.to_iced().into());
                s
            } else {
                secondary_button_style(palette, status)
            }
        })
        .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_proc_stat_with_spaces_and_parens_in_comm() {
        let sample = "1234 (Web Content (1)) S 1 1234 1234 0 -1 4194304 100 0 0 0 150 40 0 0 20 0 8 0 1000 12345678 2048 18446744073709551615 0";
        let (name, state, ticks, rss_pages, threads) = parse_proc_stat(sample).unwrap();
        assert_eq!(name, "Web Content (1)");
        assert_eq!(state, 'S');
        assert_eq!(ticks, 190);
        assert_eq!(rss_pages, 2048);
        assert_eq!(threads, 8);
    }

    #[test]
    fn formats_bytes_human_readable() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1024 * 512), "512 KB");
        assert_eq!(format_bytes(1024 * 1024 * 128), "128.0 MB");
        assert_eq!(format_bytes(1024 * 1024 * 1024 * 4), "4.00 GB");
    }

    #[test]
    fn formats_uptime_human_readable() {
        assert_eq!(format_uptime(45), "0dk 45sn");
        assert_eq!(format_uptime(125), "2dk 5sn");
        assert_eq!(format_uptime(3665), "1sa 1dk");
        assert_eq!(format_uptime(86400 + 7200), "26sa 0dk");
    }

    #[test]
    fn test_process_filtering_and_sorting() {
        let mut app = CortenMonitorApp::default();
        app.snapshot.processes = vec![
            ProcessEntry {
                pid: 100,
                name: "systemd".into(),
                state: 'S',
                cpu_percent: 0.5,
                rss_bytes: 1024 * 1024 * 10,
                threads: 1,
            },
            ProcessEntry {
                pid: 250,
                name: "corten-compositor".into(),
                state: 'R',
                cpu_percent: 15.0,
                rss_bytes: 1024 * 1024 * 80,
                threads: 8,
            },
            ProcessEntry {
                pid: 500,
                name: "firefox".into(),
                state: 'S',
                cpu_percent: 2.0,
                rss_bytes: 1024 * 1024 * 500,
                threads: 40,
            },
        ];

        // Sort by Memory Desc
        app.sort_col = ProcSortColumn::Memory;
        app.sort_desc = true;
        let list = app.filtered_processes();
        assert_eq!(list[0].name, "firefox");
        assert_eq!(list[1].name, "corten-compositor");
        assert_eq!(list[2].name, "systemd");

        // Sort by CPU Desc
        app.sort_col = ProcSortColumn::Cpu;
        app.sort_desc = true;
        let list = app.filtered_processes();
        assert_eq!(list[0].name, "corten-compositor");

        // Filter by search query
        app.search_query = "fire".into();
        let list = app.filtered_processes();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].pid, 500);

        // Filter by PID
        app.search_query = "250".into();
        let list = app.filtered_processes();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "corten-compositor");
    }

    #[test]
    fn test_proc_sampler_default_initialization() {
        let sampler = ProcSampler::default();
        assert_eq!(sampler.prev_total_ticks, 0);
        assert_eq!(sampler.prev_net_rx, 0);
        assert!(sampler.last_sample_instant.is_none());
    }
}
