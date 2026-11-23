use std::borrow::Cow;
use std::io::Write;
use std::path::PathBuf;

use corten_config::{AppearanceConfig, ConfigManager};
use corten_theme::{
    Icon, LUCIDE_FONT, LUCIDE_FONT_BYTES, NOTO_SANS, NOTO_SANS_BYTES, Palette, PatinaVariant,
    ThemeMode, Typography,
};
use iced::mouse::Button;
use iced::widget::{button, canvas, column, container, horizontal_space, image, row, stack, text};
use iced::{
    Color, Element, Event, Length, Point, Rectangle, Size, Subscription, event, keyboard, mouse,
};
use iced_layershell::Application;
use iced_layershell::actions::LayershellCustomActions;
use iced_layershell::reexport::{Anchor, KeyboardInteractivity, Layer};
use iced_layershell::settings::{LayerShellSettings, Settings};
use iced_runtime::Task;

#[derive(Debug, Clone, Default)]
pub struct SnipperFlags {
    pub pin_arg: Option<String>,
    pub delay_secs: Option<u64>,
}

pub fn detect_fullscreen_geometry() -> String {
    if let Ok(entries) = std::fs::read_dir("/sys/class/drm") {
        for entry in entries.flatten() {
            let path = entry.path();
            let status_path = path.join("status");
            let modes_path = path.join("modes");
            if status_path.exists()
                && modes_path.exists()
                && let Ok(status) = std::fs::read_to_string(&status_path)
                && status.trim() == "connected"
                && let Ok(modes) = std::fs::read_to_string(&modes_path)
                && let Some(first_mode) = modes.lines().next()
            {
                let m = first_mode.trim();
                if m.contains('x') {
                    return format!("0,0 {}\n", m);
                }
            }
        }
    }
    "0,0 1920x1080\n".to_string()
}

pub fn main() -> Result<(), iced_layershell::Error> {
    tracing_subscriber::fmt::init();

    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--fullscreen" || a == "-f") {
        let geom_str = detect_fullscreen_geometry();
        let mut stdout = std::io::stdout().lock();
        let _ = stdout.write_all(geom_str.as_bytes());
        let _ = stdout.flush();
        std::process::exit(0);
    }

    let pin_arg = args
        .iter()
        .position(|a| a == "--pin" || a == "-p")
        .and_then(|idx| args.get(idx + 1).cloned());

    let delay_secs = args
        .iter()
        .position(|a| a == "--delay" || a == "-d")
        .and_then(|idx| args.get(idx + 1))
        .and_then(|val| val.parse::<u64>().ok());

    let (anchor, size, margin, layer, interactivity) = if pin_arg.is_some() {
        (
            Anchor::Top | Anchor::Right,
            Some((560, 400)),
            (32, 32, 0, 0),
            Layer::Top,
            KeyboardInteractivity::OnDemand,
        )
    } else {
        (
            Anchor::Top | Anchor::Bottom | Anchor::Left | Anchor::Right,
            Some((0, 0)),
            (0, 0, 0, 0),
            Layer::Overlay,
            KeyboardInteractivity::Exclusive,
        )
    };

    CortenSnipperApp::run(Settings {
        layer_settings: LayerShellSettings {
            size,
            exclusive_zone: 0,
            anchor,
            layer,
            margin,
            keyboard_interactivity: interactivity,
            binded_output_name: None,
        },
        flags: SnipperFlags {
            pin_arg,
            delay_secs,
        },
        fonts: vec![
            Cow::Borrowed(LUCIDE_FONT_BYTES),
            Cow::Borrowed(NOTO_SANS_BYTES),
        ],
        default_font: NOTO_SANS,
        ..Default::default()
    })
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum SnipperState {
    Idle,
    Selecting { start: Point, current: Point },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AnnotationTool {
    #[default]
    Redact,
    BlurPixelate,
    RectangleBox,
    Arrow,
    Highlight,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AnnotationItem {
    pub tool: AnnotationTool,
    pub start: Point,
    pub end: Point,
}

impl AnnotationItem {
    pub fn bounding_rect(&self) -> Rectangle {
        let min_x = self.start.x.min(self.end.x);
        let min_y = self.start.y.min(self.end.y);
        let width = (self.start.x - self.end.x).abs();
        let height = (self.start.y - self.end.y).abs();
        Rectangle {
            x: min_x,
            y: min_y,
            width,
            height,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    PointerPressed(Point, Option<Size>),
    PointerMoved(Point),
    PointerReleased(Point),
    Cancel,
    ClosePinned,
    CountdownTick,
    PeriodicTick,
    SkipCountdown,
    SetDelay(u64),
    ToggleRedactionMode,
    SelectAnnotationTool(AnnotationTool),
    UndoRedaction,
    RedoRedaction,
    SaveRedactedImage,
    CopyImageToClipboard,
    LayerAction(LayershellCustomActions),
}

impl TryInto<LayershellCustomActions> for Message {
    type Error = Self;

    fn try_into(self) -> Result<LayershellCustomActions, Self::Error> {
        match self {
            Self::LayerAction(action) => Ok(action),
            _ => Err(self),
        }
    }
}

pub fn calculate_contain_image_rect(img_size: Size, bounds: Size) -> (Rectangle, f32) {
    if img_size.width <= 0.0
        || img_size.height <= 0.0
        || bounds.width <= 0.0
        || bounds.height <= 0.0
    {
        return (Rectangle::new(Point::ORIGIN, bounds), 1.0);
    }
    let scale = (bounds.width / img_size.width).min(bounds.height / img_size.height);
    let rendered_w = img_size.width * scale;
    let rendered_h = img_size.height * scale;
    let offset_x = (bounds.width - rendered_w) / 2.0;
    let offset_y = (bounds.height - rendered_h) / 2.0;

    (
        Rectangle {
            x: offset_x,
            y: offset_y,
            width: rendered_w,
            height: rendered_h,
        },
        scale,
    )
}

pub fn apply_annotations_and_save(
    src_path: &str,
    annotations: &[AnnotationItem],
    bounds: Size,
) -> Result<String, String> {
    let mut img = image_crate::open(src_path).map_err(|e| e.to_string())?;
    let (img_w, img_h) = (img.width() as f32, img.height() as f32);

    let (img_rect, scale) = calculate_contain_image_rect(Size::new(img_w, img_h), bounds);

    use image_crate::{GenericImage, GenericImageView, Rgba};
    for item in annotations {
        let r = item.bounding_rect();
        let x0 = (((r.x - img_rect.x) / scale).max(0.0) as u32).min(img.width());
        let y0 = (((r.y - img_rect.y) / scale).max(0.0) as u32).min(img.height());
        let x1 = (((r.x + r.width - img_rect.x) / scale).max(0.0) as u32).min(img.width());
        let y1 = (((r.y + r.height - img_rect.y) / scale).max(0.0) as u32).min(img.height());

        match item.tool {
            AnnotationTool::Redact => {
                let dark_pixel = Rgba([16, 18, 22, 255]);
                for y in y0..y1 {
                    for x in x0..x1 {
                        if x < img.width() && y < img.height() {
                            img.put_pixel(x, y, dark_pixel);
                        }
                    }
                }
            }
            AnnotationTool::BlurPixelate => {
                let block = 10u32;
                let mut by = y0;
                while by < y1 {
                    let mut bx = x0;
                    while bx < x1 {
                        let sample_x = (bx + block / 2).min(img.width().saturating_sub(1));
                        let sample_y = (by + block / 2).min(img.height().saturating_sub(1));
                        let px = img.get_pixel(sample_x, sample_y);
                        for yy in by..(by + block).min(y1) {
                            for xx in bx..(bx + block).min(x1) {
                                img.put_pixel(xx, yy, px);
                            }
                        }
                        bx += block;
                    }
                    by += block;
                }
            }
            AnnotationTool::Highlight => {
                for y in y0..y1 {
                    for x in x0..x1 {
                        if x < img.width() && y < img.height() {
                            let orig = img.get_pixel(x, y).0;
                            let blended = Rgba([
                                ((orig[0] as u16 * 3 + 250) / 4) as u8,
                                ((orig[1] as u16 * 3 + 215) / 4) as u8,
                                ((orig[2] as u16 * 3 + 40) / 4) as u8,
                                255,
                            ]);
                            img.put_pixel(x, y, blended);
                        }
                    }
                }
            }
            AnnotationTool::RectangleBox => {
                let accent = Rgba([222, 107, 72, 255]);
                for y in y0..y1 {
                    for x in x0..x1 {
                        let on_border = (x - x0) < 3
                            || (x1.saturating_sub(x)) <= 3
                            || (y - y0) < 3
                            || (y1.saturating_sub(y)) <= 3;
                        if on_border && x < img.width() && y < img.height() {
                            img.put_pixel(x, y, accent);
                        }
                    }
                }
            }
            AnnotationTool::Arrow => {
                let accent = Rgba([222, 107, 72, 255]);
                let sx = ((item.start.x - img_rect.x) / scale).clamp(0.0, img_w - 1.0);
                let sy = ((item.start.y - img_rect.y) / scale).clamp(0.0, img_h - 1.0);
                let ex = ((item.end.x - img_rect.x) / scale).clamp(0.0, img_w - 1.0);
                let ey = ((item.end.y - img_rect.y) / scale).clamp(0.0, img_h - 1.0);
                let draw_thick_line =
                    |img: &mut image_crate::DynamicImage, ax: f32, ay: f32, bx: f32, by: f32| {
                        let dist = ((bx - ax).hypot(by - ay)).max(1.0);
                        let steps = (dist * 1.5) as u32;
                        for s in 0..=steps {
                            let t = s as f32 / steps.max(1) as f32;
                            let px = (ax + (bx - ax) * t).round() as i32;
                            let py = (ay + (by - ay) * t).round() as i32;
                            for dy in -1..=1 {
                                for dx in -1..=1 {
                                    let xx = px + dx;
                                    let yy = py + dy;
                                    if xx >= 0
                                        && yy >= 0
                                        && (xx as u32) < img.width()
                                        && (yy as u32) < img.height()
                                    {
                                        img.put_pixel(xx as u32, yy as u32, accent);
                                    }
                                }
                            }
                        }
                    };
                draw_thick_line(&mut img, sx, sy, ex, ey);
                let angle = (ey - sy).atan2(ex - sx);
                let head_len = (16.0 / scale.max(0.25)).clamp(8.0, 28.0);
                let wing1_x = ex - head_len * (angle - 0.45).cos();
                let wing1_y = ey - head_len * (angle - 0.45).sin();
                let wing2_x = ex - head_len * (angle + 0.45).cos();
                let wing2_y = ey - head_len * (angle + 0.45).sin();
                draw_thick_line(&mut img, ex, ey, wing1_x, wing1_y);
                draw_thick_line(&mut img, ex, ey, wing2_x, wing2_y);
            }
        }
    }

    let p = PathBuf::from(src_path);
    let stem = p
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("screenshot");
    let ext = p.extension().and_then(|s| s.to_str()).unwrap_or("png");
    let parent = p.parent().unwrap_or_else(|| std::path::Path::new("."));
    let target = parent.join(format!("{}_annotated.{}", stem, ext));

    img.save(&target).map_err(|e| e.to_string())?;
    Ok(target.to_string_lossy().to_string())
}

pub fn copy_image_to_clipboard(path: &str) -> Result<(), String> {
    let data = std::fs::read(path).map_err(|e| e.to_string())?;
    let mut child = std::process::Command::new("wl-copy")
        .args(["--type", "image/png"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;

    if let Some(mut stdin) = child.stdin.take() {
        let _ = std::io::Write::write_all(&mut stdin, &data);
    }
    let _ = child.wait();
    Ok(())
}

struct CortenSnipperApp {
    state: SnipperState,
    pinned_image: Option<(String, image::Handle)>,
    last_saved_path: Option<String>,
    canvas_size: Size,
    theme_mode: ThemeMode,
    active_patina: PatinaVariant,
    countdown: Option<u64>,
    redaction_mode: bool,
    active_tool: AnnotationTool,
    redactions: Vec<AnnotationItem>,
    redo_stack: Vec<AnnotationItem>,
    redacting_start: Option<Point>,
    redacting_current: Option<Point>,
    status_msg: Option<String>,
}

impl Application for CortenSnipperApp {
    type Message = Message;
    type Flags = SnipperFlags;
    type Theme = iced::Theme;
    type Executor = iced::executor::Default;

    fn new(flags: Self::Flags) -> (Self, Task<Self::Message>) {
        let pinned_image = flags.pin_arg.and_then(|p| {
            let path = PathBuf::from(&p);
            if path.exists() {
                Some((p, image::Handle::from_path(path)))
            } else {
                None
            }
        });

        let active_patina = ConfigManager::<AppearanceConfig>::load_or_default("appearance")
            .map(|c| PatinaVariant::from_code(&c.get().patina))
            .unwrap_or(PatinaVariant::Rust);

        (
            Self {
                state: SnipperState::Idle,
                pinned_image,
                last_saved_path: None,
                canvas_size: Size::new(520.0, 320.0),
                theme_mode: ThemeMode::Dark,
                active_patina,
                countdown: flags.delay_secs,
                redaction_mode: false,
                active_tool: AnnotationTool::Redact,
                redactions: Vec::new(),
                redo_stack: Vec::new(),
                redacting_start: None,
                redacting_current: None,
                status_msg: None,
            },
            Task::none(),
        )
    }

    fn namespace(&self) -> String {
        "corten-snipper".to_string()
    }

    fn subscription(&self) -> Subscription<Self::Message> {
        let key_sub = event::listen_with(|event, _status, _window| match event {
            Event::Mouse(mouse::Event::ButtonPressed(Button::Left)) => None,
            Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) => match key {
                keyboard::Key::Named(keyboard::key::Named::Escape) => Some(Message::Cancel),
                keyboard::Key::Character(ref c)
                    if (c == "w" || c == "W") && modifiers.control() =>
                {
                    Some(Message::ClosePinned)
                }
                keyboard::Key::Character(ref c) if c == "q" || c == "Q" => {
                    Some(Message::ClosePinned)
                }
                keyboard::Key::Character(ref c)
                    if (c == "z" || c == "Z") && modifiers.control() =>
                {
                    Some(Message::UndoRedaction)
                }
                keyboard::Key::Character(ref c)
                    if (c == "y" || c == "Y") && modifiers.control() =>
                {
                    Some(Message::RedoRedaction)
                }
                keyboard::Key::Character(ref c)
                    if (c == "s" || c == "S") && modifiers.control() =>
                {
                    Some(Message::SaveRedactedImage)
                }
                keyboard::Key::Character(ref c)
                    if (c == "c" || c == "C") && modifiers.control() =>
                {
                    Some(Message::CopyImageToClipboard)
                }
                keyboard::Key::Character(ref c) if c == "r" || c == "R" => {
                    Some(Message::ToggleRedactionMode)
                }
                _ => None,
            },
            _ => None,
        });

        let mut subs = vec![key_sub];
        if self.countdown.is_some() {
            subs.push(
                iced::time::every(std::time::Duration::from_secs(1))
                    .map(|_| Message::CountdownTick),
            );
        }
        if self.pinned_image.is_some() {
            subs.push(
                iced::time::every(std::time::Duration::from_secs(1)).map(|_| Message::PeriodicTick),
            );
        }

        Subscription::batch(subs)
    }

    fn update(&mut self, message: Self::Message) -> Task<Self::Message> {
        match message {
            Message::PeriodicTick => {
                if let Ok(cfg) = ConfigManager::<AppearanceConfig>::load_or_default("appearance") {
                    let new_patina = PatinaVariant::from_code(&cfg.get().patina);
                    if self.active_patina != new_patina {
                        self.active_patina = new_patina;
                    }
                }
            }
            Message::CountdownTick => {
                if let Some(c) = self.countdown {
                    if c <= 1 {
                        self.countdown = None;
                    } else {
                        self.countdown = Some(c - 1);
                    }
                }
            }
            Message::SkipCountdown => {
                self.countdown = None;
            }
            Message::SetDelay(secs) => {
                self.countdown = Some(secs);
            }
            Message::ToggleRedactionMode => {
                self.redaction_mode = !self.redaction_mode;
                self.status_msg = Some(if self.redaction_mode {
                    "Çizim & Gizleme modu aktif. Aracı seçip görsel üzerinde sürükleyin.".into()
                } else {
                    "Normal önizleme moduna dönüldü.".into()
                });
            }
            Message::SelectAnnotationTool(tool) => {
                self.active_tool = tool;
                self.redaction_mode = true;
            }
            Message::UndoRedaction => {
                if let Some(last) = self.redactions.pop() {
                    self.redo_stack.push(last);
                    self.last_saved_path = None;
                    self.status_msg = Some("Son çizim geri alındı (Ctrl+Y ile yinele).".into());
                }
            }
            Message::RedoRedaction => {
                if let Some(item) = self.redo_stack.pop() {
                    self.redactions.push(item);
                    self.last_saved_path = None;
                    self.status_msg = Some("Çizim yeniden uygulandı.".into());
                }
            }
            Message::SaveRedactedImage => {
                if let Some((ref path_str, _)) = self.pinned_image {
                    let bounds = if self.canvas_size.width > 0.0 && self.canvas_size.height > 0.0 {
                        self.canvas_size
                    } else {
                        Size::new(520.0, 320.0)
                    };
                    match apply_annotations_and_save(path_str, &self.redactions, bounds) {
                        Ok(target) => {
                            self.last_saved_path = Some(target.clone());
                            self.status_msg = Some(format!("Kaydedildi: {}", target));
                        }
                        Err(err) => {
                            self.status_msg = Some(format!("Hata: {}", err));
                        }
                    }
                }
            }
            Message::CopyImageToClipboard => {
                if !self.redactions.is_empty()
                    && self.last_saved_path.is_none()
                    && let Some((ref path_str, _)) = self.pinned_image
                {
                    let bounds = if self.canvas_size.width > 0.0 && self.canvas_size.height > 0.0 {
                        self.canvas_size
                    } else {
                        Size::new(520.0, 320.0)
                    };
                    if let Ok(target) =
                        apply_annotations_and_save(path_str, &self.redactions, bounds)
                    {
                        self.last_saved_path = Some(target);
                    }
                }
                let target_path = self
                    .last_saved_path
                    .as_deref()
                    .or_else(|| self.pinned_image.as_ref().map(|(p, _)| p.as_str()));
                if let Some(path) = target_path {
                    match copy_image_to_clipboard(path) {
                        Ok(()) => {
                            self.status_msg = Some("Resim panoya kopyalandı!".into());
                        }
                        Err(err) => {
                            self.status_msg = Some(format!("Kopyalama hatası: {}", err));
                        }
                    }
                }
            }
            Message::PointerPressed(p, maybe_size) => {
                if let Some(size) = maybe_size {
                    self.canvas_size = size;
                }
                if self.pinned_image.is_some() {
                    if self.redaction_mode {
                        self.redacting_start = Some(p);
                        self.redacting_current = Some(p);
                    }
                } else {
                    self.state = SnipperState::Selecting {
                        start: p,
                        current: p,
                    };
                }
            }
            Message::PointerMoved(p) => {
                if self.pinned_image.is_some() {
                    if self.redaction_mode && self.redacting_start.is_some() {
                        self.redacting_current = Some(p);
                    }
                } else if let SnipperState::Selecting { start, .. } = self.state {
                    self.state = SnipperState::Selecting { start, current: p };
                }
            }
            Message::PointerReleased(end) => {
                if self.pinned_image.is_some() {
                    if self.redaction_mode {
                        if let Some(start) = self.redacting_start.take() {
                            let w = (start.x - end.x).abs();
                            let h = (start.y - end.y).abs();
                            if w > 4.0 || h > 4.0 {
                                self.redactions.push(AnnotationItem {
                                    tool: self.active_tool,
                                    start,
                                    end,
                                });
                                self.redo_stack.clear();
                                self.last_saved_path = None;
                            }
                        }
                        self.redacting_current = None;
                    }
                } else {
                    if let SnipperState::Selecting { start, .. } = self.state {
                        let min_x = start.x.min(end.x).round() as i32;
                        let min_y = start.y.min(end.y).round() as i32;
                        let w = (start.x - end.x).abs().round() as i32;
                        let h = (start.y - end.y).abs().round() as i32;

                        if w > 4 && h > 4 {
                            corten_services::acoustic::play(
                                corten_services::acoustic::AcousticCue::Shutter,
                            );
                            // Standard slurp geometry format: "X,Y WxH"
                            let geom_str = format!("{},{} {}x{}\n", min_x, min_y, w, h);
                            let mut stdout = std::io::stdout().lock();
                            let _ = stdout.write_all(geom_str.as_bytes());
                            let _ = stdout.flush();
                            std::process::exit(0);
                        }
                    }
                    self.state = SnipperState::Idle;
                }
            }
            Message::ClosePinned => {
                if self.pinned_image.is_some() {
                    std::process::exit(0);
                }
            }
            Message::Cancel => {
                if self.pinned_image.is_some() {
                    std::process::exit(0);
                } else {
                    std::process::exit(1);
                }
            }
            Message::LayerAction(_) => {}
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Self::Message> {
        let palette = Palette::for_mode(self.theme_mode).with_patina(self.active_patina);

        // Countdown timer view
        if let Some(c) = self.countdown {
            let countdown_card = container(
                column![
                    text(format!("{}", c))
                        .size(64.0)
                        .color(palette.primary.to_iced()),
                    text("Ekran kırpıcı açılıyor...")
                        .size(Typography::BODY)
                        .color(palette.text_primary.to_iced()),
                    row![
                        button(
                            text("Hemen Kırp")
                                .size(Typography::CAPTION)
                                .color(palette.on_primary().to_iced()),
                        )
                        .padding([6, 14])
                        .style(move |_, status| {
                            let mut s =
                                corten_theme::iced_styles::primary_button_style(palette, status);
                            s.border.radius =
                                iced::border::Radius::from(corten_theme::Radius::FULL);
                            s
                        })
                        .on_press(Message::SkipCountdown),
                        button(
                            text("İptal (Esc)")
                                .size(Typography::CAPTION)
                                .color(palette.text_muted.to_iced()),
                        )
                        .padding([6, 14])
                        .style(move |_, status| {
                            let mut s =
                                corten_theme::iced_styles::m3_tonal_button(palette, false, status);
                            s.border.radius =
                                iced::border::Radius::from(corten_theme::Radius::FULL);
                            s
                        })
                        .on_press(Message::Cancel),
                    ]
                    .spacing(8)
                    .align_y(iced::Alignment::Center),
                ]
                .spacing(12)
                .align_x(iced::Alignment::Center),
            )
            .padding([24, 32])
            .style(move |_| corten_theme::iced_styles::m3_dialog_style(palette));

            return container(countdown_card)
                .width(Length::Fill)
                .height(Length::Fill)
                .center_x(Length::Fill)
                .center_y(Length::Fill)
                .style(|_| container::Style {
                    background: Some(Color::from_rgba(0.0, 0.0, 0.0, 0.35).into()),
                    ..Default::default()
                })
                .into();
        }

        // Pinned image view with redaction / censor tools
        if let Some((ref path_str, ref handle)) = self.pinned_image {
            let filename = std::path::Path::new(path_str)
                .file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_else(|| "Ekran Görüntüsü".to_string());

            let mut header = row![
                row![
                    text(Icon::PIN)
                        .font(LUCIDE_FONT)
                        .size(12.0)
                        .color(palette.primary.to_iced()),
                    text(filename)
                        .size(Typography::CAPTION)
                        .color(palette.primary.to_iced()),
                ]
                .spacing(4)
                .align_y(iced::Alignment::Center),
                horizontal_space(),
                button(
                    row![
                        text(Icon::PEN).font(LUCIDE_FONT).size(11.0).color(
                            if self.redaction_mode {
                                palette.primary.to_iced()
                            } else {
                                palette.text_muted.to_iced()
                            }
                        ),
                        text(if self.redaction_mode {
                            "Düzenle: Açık"
                        } else {
                            "Çiz & Gizle"
                        })
                        .size(11.0)
                        .color(if self.redaction_mode {
                            palette.primary.to_iced()
                        } else {
                            palette.text_muted.to_iced()
                        }),
                    ]
                    .spacing(4)
                    .align_y(iced::Alignment::Center),
                )
                .padding([2, 8])
                .style(move |_, _| button::Style {
                    background: Some(if self.redaction_mode {
                        Color::from_rgba(
                            palette.primary.r,
                            palette.primary.g,
                            palette.primary.b,
                            0.25,
                        )
                        .into()
                    } else {
                        Color::from_rgba(1.0, 1.0, 1.0, 0.06).into()
                    }),
                    border: iced::Border {
                        radius: iced::border::Radius::from(6.0),
                        width: 1.0,
                        color: if self.redaction_mode {
                            palette.primary.to_iced()
                        } else {
                            Color::TRANSPARENT
                        },
                    },
                    ..Default::default()
                })
                .on_press(Message::ToggleRedactionMode),
                button(
                    text(Icon::LOCK)
                        .font(LUCIDE_FONT)
                        .size(12.0)
                        .color(palette.text_primary.to_iced())
                )
                .padding([3, 6])
                .on_press(Message::SelectAnnotationTool(AnnotationTool::Redact)),
                button(
                    text(Icon::DROPLET)
                        .font(LUCIDE_FONT)
                        .size(12.0)
                        .color(palette.text_primary.to_iced())
                )
                .padding([3, 6])
                .on_press(Message::SelectAnnotationTool(AnnotationTool::BlurPixelate)),
                button(
                    text(Icon::SQUARE)
                        .font(LUCIDE_FONT)
                        .size(12.0)
                        .color(palette.text_primary.to_iced())
                )
                .padding([3, 6])
                .on_press(Message::SelectAnnotationTool(AnnotationTool::RectangleBox)),
                button(
                    text(Icon::ARROW_UP_RIGHT)
                        .font(LUCIDE_FONT)
                        .size(12.0)
                        .color(palette.text_primary.to_iced())
                )
                .padding([3, 6])
                .on_press(Message::SelectAnnotationTool(AnnotationTool::Arrow)),
                button(
                    text(Icon::PEN)
                        .font(LUCIDE_FONT)
                        .size(12.0)
                        .color(palette.warning.to_iced())
                )
                .padding([3, 6])
                .on_press(Message::SelectAnnotationTool(AnnotationTool::Highlight)),
            ]
            .spacing(6)
            .align_y(iced::Alignment::Center);

            if !self.redactions.is_empty() {
                header = header.push(
                    button(
                        row![
                            text(Icon::UNDO)
                                .font(LUCIDE_FONT)
                                .size(11.0)
                                .color(palette.text_muted.to_iced()),
                            text("Geri Al")
                                .size(11.0)
                                .color(palette.text_muted.to_iced()),
                        ]
                        .spacing(4)
                        .align_y(iced::Alignment::Center),
                    )
                    .padding([2, 6])
                    .style(|_, _| button::Style::default())
                    .on_press(Message::UndoRedaction),
                );
                header = header.push(
                    button(
                        row![
                            text(Icon::SAVE)
                                .font(LUCIDE_FONT)
                                .size(11.0)
                                .color(palette.primary.to_iced()),
                            text("Kaydet").size(11.0).color(palette.primary.to_iced()),
                        ]
                        .spacing(4)
                        .align_y(iced::Alignment::Center),
                    )
                    .padding([2, 8])
                    .style(move |_, _| button::Style {
                        background: Some(
                            Color::from_rgba(
                                palette.primary.r,
                                palette.primary.g,
                                palette.primary.b,
                                0.20,
                            )
                            .into(),
                        ),
                        border: iced::Border {
                            radius: iced::border::Radius::from(6.0),
                            ..Default::default()
                        },
                        ..Default::default()
                    })
                    .on_press(Message::SaveRedactedImage),
                );
            }

            header = header.push(
                button(
                    row![
                        text(Icon::COPY)
                            .font(LUCIDE_FONT)
                            .size(11.0)
                            .color(palette.text_primary.to_iced()),
                        text("Kopyala")
                            .size(11.0)
                            .color(palette.text_primary.to_iced()),
                    ]
                    .spacing(4)
                    .align_y(iced::Alignment::Center),
                )
                .padding([2, 8])
                .style(move |_, _| button::Style {
                    background: Some(Color::from_rgba(1.0, 1.0, 1.0, 0.08).into()),
                    border: iced::Border {
                        radius: iced::border::Radius::from(6.0),
                        ..Default::default()
                    },
                    ..Default::default()
                })
                .on_press(Message::CopyImageToClipboard),
            );

            header = header.push(
                button(
                    text(Icon::X)
                        .font(LUCIDE_FONT)
                        .size(12.0)
                        .color(palette.text_muted.to_iced()),
                )
                .padding([2, 6])
                .style(|_, _| button::Style::default())
                .on_press(Message::ClosePinned),
            );

            let img_widget = image(handle.clone())
                .width(Length::Fill)
                .height(Length::Fill)
                .content_fit(iced::ContentFit::Contain);

            let redaction_canvas = canvas(RedactionCanvas {
                redactions: self.redactions.clone(),
                active_start: self.redacting_start,
                active_current: self.redacting_current,
                palette,
            })
            .width(Length::Fill)
            .height(Length::Fill);

            let image_area = stack![img_widget, redaction_canvas];

            let mut body_col = column![header, image_area].spacing(6);
            if let Some(ref msg) = self.status_msg {
                body_col = body_col.push(text(msg).size(11.0).color(palette.primary.to_iced()));
            }

            let pinned_box = container(body_col)
                .padding([8, 12])
                .width(Length::Fill)
                .height(Length::Fill)
                .style(move |_| {
                    corten_theme::iced_styles::m3_surface_card(palette, corten_theme::Radius::LG)
                });

            return container(pinned_box)
                .padding(8)
                .width(Length::Fill)
                .height(Length::Fill)
                .into();
        }

        // Selection overlay view
        let canvas_widget = canvas(SelectionCanvas {
            state: self.state,
            palette,
        })
        .width(Length::Fill)
        .height(Length::Fill);

        let hint = container(
            row![
                text("Ekran Kırpıcı — Çerçeveyi sürükleyin • Esc: İptal")
                    .size(Typography::BODY_SM)
                    .color(palette.text_primary.to_iced()),
                horizontal_space(),
                button(
                    text("3s Gecikme")
                        .size(Typography::CAPTION)
                        .color(palette.text_primary.to_iced())
                )
                .padding([4, 10])
                .style(move |_, status| {
                    let mut s = corten_theme::iced_styles::m3_tonal_button(palette, false, status);
                    s.border.radius = iced::border::Radius::from(corten_theme::Radius::FULL);
                    s
                })
                .on_press(Message::SetDelay(3)),
                button(
                    text("5s Gecikme")
                        .size(Typography::CAPTION)
                        .color(palette.text_primary.to_iced())
                )
                .padding([4, 10])
                .style(move |_, status| {
                    let mut s = corten_theme::iced_styles::m3_tonal_button(palette, false, status);
                    s.border.radius = iced::border::Radius::from(corten_theme::Radius::FULL);
                    s
                })
                .on_press(Message::SetDelay(5)),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center),
        )
        .padding([8, 16])
        .style(move |_| {
            corten_theme::iced_styles::m3_surface_card(palette, corten_theme::Radius::LG)
        });

        let floating_hint = container(hint)
            .width(Length::Fill)
            .padding([16, 24])
            .align_x(iced::alignment::Horizontal::Center);

        container(stack![canvas_widget, floating_hint])
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_| container::Style {
                background: Some(Color::from_rgba(0.0, 0.0, 0.0, 0.20).into()),
                ..Default::default()
            })
            .into()
    }
}

struct SelectionCanvas {
    state: SnipperState,
    palette: Palette,
}

impl canvas::Program<Message> for SelectionCanvas {
    type State = ();

    fn update(
        &self,
        _state: &mut Self::State,
        event: canvas::Event,
        _bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> (canvas::event::Status, Option<Message>) {
        match event {
            canvas::Event::Mouse(mouse::Event::ButtonPressed(Button::Left)) => {
                if let Some(pos) = _cursor.position() {
                    return (
                        canvas::event::Status::Captured,
                        Some(Message::PointerPressed(pos, None)),
                    );
                }
            }
            canvas::Event::Mouse(mouse::Event::CursorMoved { position }) => {
                return (
                    canvas::event::Status::Captured,
                    Some(Message::PointerMoved(position)),
                );
            }
            canvas::Event::Mouse(mouse::Event::ButtonReleased(Button::Left)) => {
                if let Some(pos) = _cursor.position() {
                    return (
                        canvas::event::Status::Captured,
                        Some(Message::PointerReleased(pos)),
                    );
                }
            }
            _ => {}
        }
        (canvas::event::Status::Ignored, None)
    }

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &iced::Renderer,
        _theme: &iced::Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());

        if let SnipperState::Selecting { start, current } = self.state {
            let line_color = Color::from_rgba(1.0, 1.0, 1.0, 0.35);
            frame.stroke(
                &canvas::Path::line(
                    Point::new(0.0, current.y),
                    Point::new(bounds.width, current.y),
                ),
                canvas::Stroke::default()
                    .with_color(line_color)
                    .with_width(1.0),
            );
            frame.stroke(
                &canvas::Path::line(
                    Point::new(current.x, 0.0),
                    Point::new(current.x, bounds.height),
                ),
                canvas::Stroke::default()
                    .with_color(line_color)
                    .with_width(1.0),
            );

            let min_x = start.x.min(current.x);
            let min_y = start.y.min(current.y);
            let w = (start.x - current.x).abs();
            let h = (start.y - current.y).abs();

            let rect = Rectangle {
                x: min_x,
                y: min_y,
                width: w,
                height: h,
            };

            frame.fill_rectangle(
                Point::new(rect.x, rect.y),
                Size::new(rect.width, rect.height),
                Color::from_rgba(
                    self.palette.primary.r,
                    self.palette.primary.g,
                    self.palette.primary.b,
                    0.18,
                ),
            );

            frame.stroke(
                &canvas::Path::rectangle(
                    Point::new(rect.x, rect.y),
                    Size::new(rect.width, rect.height),
                ),
                canvas::Stroke::default()
                    .with_color(self.palette.primary.to_iced())
                    .with_width(2.0),
            );

            let badge_text = canvas::Text {
                content: format!("{:.0} × {:.0}", w, h),
                position: Point::new(min_x + 8.0, (min_y - 20.0).max(4.0)),
                color: Color::WHITE,
                size: 12.0.into(),
                ..Default::default()
            };
            frame.fill_text(badge_text);
        }

        vec![frame.into_geometry()]
    }
}

struct RedactionCanvas {
    redactions: Vec<AnnotationItem>,
    active_start: Option<Point>,
    active_current: Option<Point>,
    palette: Palette,
}

impl canvas::Program<Message> for RedactionCanvas {
    type State = ();

    fn update(
        &self,
        _state: &mut Self::State,
        event: canvas::Event,
        _bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> (canvas::event::Status, Option<Message>) {
        match event {
            canvas::Event::Mouse(mouse::Event::ButtonPressed(Button::Left)) => {
                if let Some(pos) = _cursor.position() {
                    return (
                        canvas::event::Status::Captured,
                        Some(Message::PointerPressed(pos, Some(_bounds.size()))),
                    );
                }
            }
            canvas::Event::Mouse(mouse::Event::CursorMoved { position }) => {
                return (
                    canvas::event::Status::Captured,
                    Some(Message::PointerMoved(position)),
                );
            }
            canvas::Event::Mouse(mouse::Event::ButtonReleased(Button::Left)) => {
                if let Some(pos) = _cursor.position() {
                    return (
                        canvas::event::Status::Captured,
                        Some(Message::PointerReleased(pos)),
                    );
                }
            }
            _ => {}
        }
        (canvas::event::Status::Ignored, None)
    }

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &iced::Renderer,
        _theme: &iced::Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());

        // Existing annotation items
        for item in &self.redactions {
            let r = item.bounding_rect();
            match item.tool {
                AnnotationTool::Redact => {
                    frame.fill_rectangle(
                        Point::new(r.x, r.y),
                        Size::new(r.width, r.height),
                        Color::from_rgba(0.06, 0.07, 0.09, 0.96),
                    );
                    frame.stroke(
                        &canvas::Path::rectangle(
                            Point::new(r.x, r.y),
                            Size::new(r.width, r.height),
                        ),
                        canvas::Stroke::default()
                            .with_color(self.palette.primary.to_iced())
                            .with_width(1.5),
                    );
                }
                AnnotationTool::BlurPixelate => {
                    frame.fill_rectangle(
                        Point::new(r.x, r.y),
                        Size::new(r.width, r.height),
                        Color::from_rgba(0.20, 0.22, 0.28, 0.82),
                    );
                    let badge = canvas::Text {
                        content: "Bulanık".into(),
                        position: Point::new(r.x + 4.0, r.y + 4.0),
                        color: Color::from_rgba(1.0, 1.0, 1.0, 0.80),
                        size: 10.0.into(),
                        ..Default::default()
                    };
                    frame.fill_text(badge);
                }
                AnnotationTool::Highlight => {
                    frame.fill_rectangle(
                        Point::new(r.x, r.y),
                        Size::new(r.width, r.height),
                        Color::from_rgba(0.98, 0.85, 0.16, 0.35),
                    );
                }
                AnnotationTool::RectangleBox => {
                    frame.stroke(
                        &canvas::Path::rectangle(
                            Point::new(r.x, r.y),
                            Size::new(r.width, r.height),
                        ),
                        canvas::Stroke::default()
                            .with_color(self.palette.primary.to_iced())
                            .with_width(2.5),
                    );
                }
                AnnotationTool::Arrow => {
                    frame.stroke(
                        &canvas::Path::line(item.start, item.end),
                        canvas::Stroke::default()
                            .with_color(self.palette.primary.to_iced())
                            .with_width(2.5),
                    );
                }
            }
        }

        // Active dragging redaction box
        if let (Some(start), Some(curr)) = (self.active_start, self.active_current) {
            let min_x = start.x.min(curr.x);
            let min_y = start.y.min(curr.y);
            let w = (start.x - curr.x).abs();
            let h = (start.y - curr.y).abs();

            frame.fill_rectangle(
                Point::new(min_x, min_y),
                Size::new(w, h),
                Color::from_rgba(0.06, 0.07, 0.09, 0.75),
            );
            frame.stroke(
                &canvas::Path::rectangle(Point::new(min_x, min_y), Size::new(w, h)),
                canvas::Stroke::default()
                    .with_color(self.palette.warning.to_iced())
                    .with_width(1.5),
            );
        }

        vec![frame.into_geometry()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_snipper_flags_default() {
        let flags = SnipperFlags::default();
        assert!(flags.pin_arg.is_none());
        assert!(flags.delay_secs.is_none());
    }

    #[test]
    fn test_redaction_geometry_clamping() {
        let rect = Rectangle {
            x: 10.0,
            y: 20.0,
            width: 100.0,
            height: 50.0,
        };
        assert_eq!(rect.width, 100.0);
        assert_eq!(rect.height, 50.0);
    }

    #[test]
    fn test_detect_fullscreen_geometry() {
        let geom = detect_fullscreen_geometry();
        assert!(geom.starts_with("0,0 "));
        assert!(geom.ends_with('\n'));
        let dims = geom.trim_start_matches("0,0 ").trim();
        let parts: Vec<&str> = dims.split('x').collect();
        assert_eq!(parts.len(), 2);
        assert!(parts[0].parse::<u32>().is_ok());
        assert!(parts[1].parse::<u32>().is_ok());
    }

    #[test]
    fn test_calculate_contain_image_rect_letterbox() {
        // 16:9 image (1920x1080) inside 1:1 window (1000x1000)
        let (rect, scale) =
            calculate_contain_image_rect(Size::new(1920.0, 1080.0), Size::new(1000.0, 1000.0));
        assert!((rect.x - 0.0).abs() < 1e-3);
        assert!(rect.y > 0.0); // Letterboxing top and bottom
        assert!((rect.width - 1000.0).abs() < 1e-3);
        assert!((scale - (1000.0 / 1920.0)).abs() < 1e-4);
    }

    #[test]
    fn test_calculate_contain_image_rect_pillarbox() {
        // 4:3 image (800x600) inside wide 16:9 window (1600x900)
        let (rect, scale) =
            calculate_contain_image_rect(Size::new(800.0, 600.0), Size::new(1600.0, 900.0));
        assert!(rect.x > 0.0); // Pillarboxing left and right
        assert!((rect.y - 0.0).abs() < 1e-3);
        assert!((rect.height - 900.0).abs() < 1e-3);
        assert!((scale - 1.5).abs() < 1e-4);
    }

    #[test]
    fn test_slurp_geometry_format_validity() {
        let min_x = 120;
        let min_y = 80;
        let w = 450;
        let h = 300;
        let geom_str = format!("{},{} {}x{}\n", min_x, min_y, w, h);
        assert_eq!(geom_str, "120,80 450x300\n");
    }
}
