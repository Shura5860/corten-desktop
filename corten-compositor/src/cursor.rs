use std::collections::HashMap;
use std::sync::Arc;
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::input::pointer::CursorIcon;
use smithay::utils::{Rectangle, Transform};
use tracing::info;
use xcursor::parser::parse_xcursor;
use xcursor::CursorTheme;

pub struct LoadedCursor {
    pub buffer: MemoryRenderBuffer,
    pub xhot: i32,
    pub yhot: i32,
}

pub struct CursorThemeManager {
    theme: CursorTheme,
    target_size: u32,
    cache: HashMap<CursorIcon, Option<Arc<LoadedCursor>>>,
}

fn icon_names(icon: CursorIcon) -> &'static [&'static str] {
    match icon {
        CursorIcon::Default => &["default", "left_ptr", "arrow"],
        CursorIcon::ContextMenu => &["context-menu"],
        CursorIcon::Help => &["help", "question_arrow", "whats_this"],
        CursorIcon::Pointer => &["pointer", "pointing_hand", "hand2", "hand"],
        CursorIcon::Progress => &["progress", "left_ptr_watch"],
        CursorIcon::Wait => &["wait", "watch"],
        CursorIcon::Cell => &["cell", "crosshair"],
        CursorIcon::Crosshair => &["crosshair", "cross"],
        CursorIcon::Text => &["text", "xterm", "ibeam"],
        CursorIcon::VerticalText => &["vertical-text"],
        CursorIcon::Alias => &["alias"],
        CursorIcon::Copy => &["copy"],
        CursorIcon::Move => &["move", "all-scroll"],
        CursorIcon::NoDrop => &["no-drop", "circle"],
        CursorIcon::NotAllowed => &["not-allowed", "crossed_circle"],
        CursorIcon::Grab => &["grab", "openhand", "hand1"],
        CursorIcon::Grabbing => &["grabbing", "closedhand"],
        CursorIcon::AllScroll => &["all-scroll", "move"],
        CursorIcon::ColResize => &["col-resize", "h_double_arrow", "size_hor", "ew-resize"],
        CursorIcon::RowResize => &["row-resize", "v_double_arrow", "size_ver", "ns-resize"],
        CursorIcon::NResize => &["n-resize", "top_side", "v_double_arrow", "ns-resize"],
        CursorIcon::EResize => &["e-resize", "right_side", "h_double_arrow", "ew-resize"],
        CursorIcon::SResize => &["s-resize", "bottom_side", "v_double_arrow", "ns-resize"],
        CursorIcon::WResize => &["w-resize", "left_side", "h_double_arrow", "ew-resize"],
        CursorIcon::NeResize => &["ne-resize", "top_right_corner", "nesw-resize"],
        CursorIcon::NwResize => &["nw-resize", "top_left_corner", "nwse-resize"],
        CursorIcon::SeResize => &["se-resize", "bottom_right_corner", "nwse-resize"],
        CursorIcon::SwResize => &["sw-resize", "bottom_left_corner", "nesw-resize"],
        CursorIcon::EwResize => &["ew-resize", "h_double_arrow", "size_hor", "col-resize"],
        CursorIcon::NsResize => &["ns-resize", "v_double_arrow", "size_ver", "row-resize"],
        CursorIcon::NeswResize => &["nesw-resize", "fd_double_arrow", "size_bdiag"],
        CursorIcon::NwseResize => &["nwse-resize", "bd_double_arrow", "size_fdiag"],
        CursorIcon::ZoomIn => &["zoom-in"],
        CursorIcon::ZoomOut => &["zoom-out"],
        _ => &["default", "left_ptr"],
    }
}

impl Default for CursorThemeManager {
    fn default() -> Self {
        Self::new()
    }
}

impl CursorThemeManager {
    pub fn new() -> Self {
        let theme_name = std::env::var("XCURSOR_THEME").unwrap_or_else(|_| {
            if std::path::Path::new("/usr/share/icons/breeze_cursors").exists() {
                "breeze_cursors".into()
            } else if std::path::Path::new("/usr/share/icons/Adwaita").exists() {
                "Adwaita".into()
            } else {
                "default".into()
            }
        });
        info!("CursorThemeManager: Loading XCursor theme '{}'", theme_name);
        let theme = CursorTheme::load(&theme_name);
        let target_size = std::env::var("XCURSOR_SIZE")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(24);

        Self {
            theme,
            target_size,
            cache: HashMap::new(),
        }
    }

    pub fn set_target_size(&mut self, size: u32) {
        if self.target_size != size {
            self.target_size = size;
            self.cache.clear();
        }
    }

    pub fn get(&mut self, icon: CursorIcon) -> Option<Arc<LoadedCursor>> {
        if let Some(cached) = self.cache.get(&icon) {
            return cached.clone();
        }

        let names = icon_names(icon);
        let mut loaded = None;

        for &name in names {
            if let Some(path) = self.theme.load_icon(name) {
                if let Ok(bytes) = std::fs::read(&path) {
                    if let Some(images) = parse_xcursor(&bytes) {
                        if let Some(best) = images
                            .into_iter()
                            .min_by_key(|img| (img.size as i32 - self.target_size as i32).abs())
                        {
                            let w = best.width as i32;
                            let h = best.height as i32;
                            let mut buffer = MemoryRenderBuffer::new(
                                Fourcc::Argb8888,
                                (w, h),
                                1,
                                Transform::Normal,
                                None,
                            );
                            let draw_res = buffer.render().draw(|buf| {
                                buf.copy_from_slice(&best.pixels_argb);
                                Result::<_, ()>::Ok(vec![Rectangle::from_size((w, h).into())])
                            });
                            if draw_res.is_ok() {
                                loaded = Some(Arc::new(LoadedCursor {
                                    buffer,
                                    xhot: best.xhot as i32,
                                    yhot: best.yhot as i32,
                                }));
                                break;
                            }
                        }
                    }
                }
            }
        }

        self.cache.insert(icon, loaded.clone());
        loaded
    }
}
