use corten_config::{LayoutMode, WindowRuleConfig};
use serde::{Deserialize, Serialize};
use smithay::desktop::Window;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::Resource;
use smithay::utils::{IsAlive, Logical, Point, Rectangle, Size};
use smithay::wayland::compositor::with_states;
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::shell::xdg::XdgToplevelSurfaceData;
use std::collections::{HashMap, HashSet};

/// Metadata snapshot of a managed window for IPC/Panel synchronization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowSnapshot {
    pub id: u64,
    pub app_id: String,
    pub title: String,
    pub workspace: usize,
    pub is_active: bool,
    pub is_floating: bool,
}

/// Stable numeric identity for a Smithay `Window` across both Wayland toplevels and XWayland surfaces.
pub fn window_stable_id(window: &Window) -> u64 {
    if let Some(x11) = window.x11_surface() {
        return (1u64 << 32) | u64::from(x11.window_id());
    }
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    window.hash(&mut hasher);
    if let Some(toplevel) = window.toplevel() {
        let surface = toplevel.wl_surface();
        if let Some(client) = surface.client() {
            client.id().hash(&mut hasher);
        }
        surface.id().protocol_id().hash(&mut hasher);
    }
    let id = hasher.finish();
    (id & 0x7FFF_FFFF_FFFF_FFFF).max(1)
}

/// Returns the underlying `WlSurface` for either a Wayland or XWayland window.
pub fn window_wl_surface(window: &Window) -> Option<WlSurface> {
    window.wl_surface().map(|s| s.into_owned())
}

/// Requests a window to close gracefully (Wayland `xdg_toplevel.close` or X11 `WM_DELETE_WINDOW`).
pub fn close_window(window: &Window) {
    if let Some(toplevel) = window.toplevel() {
        toplevel.send_close();
    } else if let Some(x11) = window.x11_surface() {
        let _ = x11.close();
    }
}

/// Standard height of Corten's server-side titlebar in logical pixels.
pub const TITLEBAR_HEIGHT: i32 = 36;

/// Returns `true` if the compositor should draw a server-side titlebar (SSD) for this window.
///
/// - Wayland clients that explicitly negotiated `Mode::ServerSide` get SSD.
/// - Wayland clients that requested `Mode::ClientSide`, OR never bound `zxdg_toplevel_decoration_v1`
///   at all (e.g. GTK4/libadwaita, Electron, VS Code, Antigravity IDE, Brave/Chromium), draw their
///   own Client-Side Decorations (CSD), so we return `false` to prevent double titlebars.
/// - XWayland clients respect `_MOTIF_WM_HINTS` via `X11Surface::is_decorated()`.
pub fn window_uses_ssd(window: &Window) -> bool {
    if let Some(toplevel) = window.toplevel() {
        use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode;
        let pending_mode = toplevel.with_pending_state(|p| p.decoration_mode);
        let mode = pending_mode.or_else(|| toplevel.current_state().decoration_mode);
        return matches!(mode, Some(Mode::ServerSide));
    }
    if let Some(x11) = window.x11_surface() {
        return x11.is_decorated();
    }
    false
}

/// Returns the server-side titlebar height for `window` (`36` for SSD, `0` for CSD or fullscreen).
pub fn window_titlebar_height(window: &Window, is_fullscreen: bool) -> i32 {
    if is_fullscreen || !window_uses_ssd(window) {
        0
    } else {
        TITLEBAR_HEIGHT
    }
}

/// Returns the CSD shadow/content offset (`window.geometry().loc`) for CSD windows so shadows
/// do not push the visible window away from screen edges or offset pointer hit-testing.
pub fn window_csd_offset(window: &Window) -> Point<i32, Logical> {
    if window_uses_ssd(window) {
        Point::from((0, 0))
    } else {
        window.geometry().loc
    }
}

/// 8-directional window resize edge/corner identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResizeEdge {
    Top,
    Bottom,
    Left,
    Right,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl ResizeEdge {
    pub fn from_xdg(
        edge: smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::ResizeEdge,
    ) -> Self {
        use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::ResizeEdge as XdgEdge;
        match edge {
            XdgEdge::Top => Self::Top,
            XdgEdge::Bottom => Self::Bottom,
            XdgEdge::Left => Self::Left,
            XdgEdge::Right => Self::Right,
            XdgEdge::TopLeft => Self::TopLeft,
            XdgEdge::TopRight => Self::TopRight,
            XdgEdge::BottomLeft => Self::BottomLeft,
            XdgEdge::BottomRight | XdgEdge::None | _ => Self::BottomRight,
        }
    }

    pub fn from_x11(edge: smithay::xwayland::xwm::ResizeEdge) -> Self {
        use smithay::xwayland::xwm::ResizeEdge as X11Edge;
        match edge {
            X11Edge::Top => Self::Top,
            X11Edge::Bottom => Self::Bottom,
            X11Edge::Left => Self::Left,
            X11Edge::Right => Self::Right,
            X11Edge::TopLeft => Self::TopLeft,
            X11Edge::TopRight => Self::TopRight,
            X11Edge::BottomLeft => Self::BottomLeft,
            X11Edge::BottomRight => Self::BottomRight,
        }
    }
}

/// Detects whether `cursor` lies on any of the 8 resize borders/corners of `rect`.
pub fn hit_test_resize_edge(
    rect: Rectangle<i32, Logical>,
    cursor: Point<f64, Logical>,
    border_px: f64,
) -> Option<ResizeEdge> {
    let rel_x = cursor.x - rect.loc.x as f64;
    let rel_y = cursor.y - rect.loc.y as f64;
    let w = rect.size.w as f64;
    let h = rect.size.h as f64;
    if rel_x < -2.0 || rel_y < -2.0 || rel_x > w + 2.0 || rel_y > h + 2.0 {
        return None;
    }

    let corner = border_px * 2.0;
    let left = rel_x <= border_px;
    let right = rel_x >= w - border_px;
    let top = rel_y <= border_px;
    let bottom = rel_y >= h - border_px;

    if (rel_x <= corner && rel_y <= corner) && (left || top) {
        Some(ResizeEdge::TopLeft)
    } else if (rel_x >= w - corner && rel_y <= corner) && (right || top) {
        Some(ResizeEdge::TopRight)
    } else if (rel_x <= corner && rel_y >= h - corner) && (left || bottom) {
        Some(ResizeEdge::BottomLeft)
    } else if (rel_x >= w - corner && rel_y >= h - corner) && (right || bottom) {
        Some(ResizeEdge::BottomRight)
    } else if left {
        Some(ResizeEdge::Left)
    } else if right {
        Some(ResizeEdge::Right)
    } else if top {
        Some(ResizeEdge::Top)
    } else if bottom {
        Some(ResizeEdge::Bottom)
    } else {
        None
    }
}

/// Computes the new window frame rectangle during an interactive 8-directional resize,
/// properly moving `loc.x`/`loc.y` for left/top edges and honoring both `min_size` and `max_size`.
pub fn compute_resized_rect(
    initial_rect: Rectangle<i32, Logical>,
    edge: ResizeEdge,
    delta: Point<f64, Logical>,
    min_size: (i32, i32),
    max_size: (Option<i32>, Option<i32>),
    titlebar_h: i32,
) -> Rectangle<i32, Logical> {
    let dx = delta.x.round() as i32;
    let dy = delta.y.round() as i32;

    let min_w = min_size.0.max(160);
    let min_h = (min_size.1 + titlebar_h).max(100 + titlebar_h);
    let max_w = max_size.0.map(|m| m.max(min_w));
    let max_h = max_size.1.map(|m| (m + titlebar_h).max(min_h));

    let clamp_w = |w: i32| {
        let w = w.max(min_w);
        if let Some(mw) = max_w { w.min(mw) } else { w }
    };
    let clamp_h = |h: i32| {
        let h = h.max(min_h);
        if let Some(mh) = max_h { h.min(mh) } else { h }
    };

    let mut x = initial_rect.loc.x;
    let mut y = initial_rect.loc.y;
    let mut w = initial_rect.size.w;
    let mut h = initial_rect.size.h;

    let right_edge = initial_rect.loc.x + initial_rect.size.w;
    let bottom_edge = initial_rect.loc.y + initial_rect.size.h;

    match edge {
        ResizeEdge::Right | ResizeEdge::TopRight | ResizeEdge::BottomRight => {
            w = clamp_w(initial_rect.size.w + dx);
        }
        ResizeEdge::Left | ResizeEdge::TopLeft | ResizeEdge::BottomLeft => {
            w = clamp_w(initial_rect.size.w - dx);
            x = right_edge - w;
        }
        _ => {}
    }

    match edge {
        ResizeEdge::Bottom | ResizeEdge::BottomLeft | ResizeEdge::BottomRight => {
            h = clamp_h(initial_rect.size.h + dy);
        }
        ResizeEdge::Top | ResizeEdge::TopLeft | ResizeEdge::TopRight => {
            h = clamp_h(initial_rect.size.h - dy);
            y = bottom_edge - h;
        }
        _ => {}
    }

    Rectangle::new(Point::from((x, y)), Size::from((w, h)))
}

/// Configures a window's logical geometry for either Wayland or XWayland surfaces.
pub fn configure_window_geometry(window: &Window, rect: Rectangle<i32, Logical>) {
    configure_window_geometry_ex(window, rect, None, false);
}

pub fn configure_window_geometry_force(
    window: &Window,
    rect: Rectangle<i32, Logical>,
    maximized: Option<bool>,
) {
    configure_window_geometry_ex(window, rect, maximized, true);
}

fn configure_window_geometry_ex(
    window: &Window,
    rect: Rectangle<i32, Logical>,
    maximized: Option<bool>,
    force_send: bool,
) {
    if let Some(toplevel) = window.toplevel() {
        let uses_ssd = window_uses_ssd(window);
        let needs_configure = toplevel.with_pending_state(|pending| {
            use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State;
            let is_fs = pending.states.contains(State::Fullscreen);
            let titlebar_h = if is_fs || !uses_ssd { 0 } else { TITLEBAR_HEIGHT };
            let client_h = (rect.size.h - titlebar_h).max(100);
            let new_size = Some((rect.size.w.max(160), client_h).into());
            let mut changed = force_send;
            if pending.size != new_size {
                pending.size = new_size;
                changed = true;
            }
            if pending.bounds != new_size {
                pending.bounds = new_size;
                changed = true;
            }
            if let Some(max) = maximized {
                let was_max = pending.states.contains(State::Maximized);
                if max && !was_max {
                    pending.states.set(State::Maximized);
                    changed = true;
                } else if !max && was_max {
                    pending.states.unset(State::Maximized);
                    changed = true;
                }
            }
            changed
        });
        if needs_configure {
            toplevel.send_configure();
        }
    } else if let Some(x11) = window.x11_surface() {
        let titlebar_h = window_titlebar_height(window, x11.is_fullscreen());
        let x11_rect = Rectangle::new(
            Point::from((rect.loc.x, rect.loc.y + titlebar_h)),
            Size::from((rect.size.w.max(160), (rect.size.h - titlebar_h).max(100))),
        );
        if force_send || x11.geometry() != x11_rect {
            let _ = x11.configure(x11_rect);
        }
    }
}

/// Extracts `(app_id, title)` from an XDG toplevel or XWayland `X11Surface`.
pub fn window_metadata(window: &Window) -> (String, String) {
    if let Some(x11) = window.x11_surface() {
        return (x11.class(), x11.title());
    }
    let Some(toplevel) = window.toplevel() else {
        return (String::new(), String::new());
    };
    with_states(toplevel.wl_surface(), |states| {
        let role = states
            .data_map
            .get::<XdgToplevelSurfaceData>()
            .and_then(|data| data.lock().ok());
        let app_id = role
            .as_ref()
            .and_then(|r| r.app_id.clone())
            .unwrap_or_default();
        let title = role
            .as_ref()
            .and_then(|r| r.title.clone())
            .unwrap_or_default();
        (app_id, title)
    })
}

/// Returns true if the window is alive and not a dummy/hidden winit boot surface.
pub fn is_window_mapped(window: &Window) -> bool {
    let (app_id, title) = window_metadata(window);
    if title == "winit window" || app_id == "winit window" {
        return false;
    }
    window.alive()
}

/// Retrieves the requested minimum dimensions (width, height) of a window.
pub fn window_min_size(window: &Window) -> (i32, i32) {
    if let Some(x11) = window.x11_surface() {
        if let Some(hints) = x11.size_hints() {
            if let Some(min) = hints.min_size {
                return (min.0.max(160), min.1.max(100));
            }
        }
    }
    if let Some(toplevel) = window.toplevel() {
        let (min_w, min_h) = with_states(toplevel.wl_surface(), |states| {
            let mut guard = states
                .cached_state
                .get::<smithay::wayland::shell::xdg::SurfaceCachedState>();
            let cached = guard.current();
            (cached.min_size.w.max(0), cached.min_size.h.max(0))
        });
        if min_w > 0 || min_h > 0 {
            return (min_w.max(200), min_h.max(140));
        }
    }
    (200, 140)
}

/// Retrieves the requested maximum dimensions `(max_w, max_h)` of a window (`None` = unconstrained).
pub fn window_max_size(window: &Window) -> (Option<i32>, Option<i32>) {
    if let Some(x11) = window.x11_surface() {
        if let Some(hints) = x11.size_hints() {
            if let Some(max) = hints.max_size {
                return (
                    (max.0 > 0).then_some(max.0),
                    (max.1 > 0).then_some(max.1),
                );
            }
        }
    }
    if let Some(toplevel) = window.toplevel() {
        let (max_w, max_h) = with_states(toplevel.wl_surface(), |states| {
            let mut guard = states
                .cached_state
                .get::<smithay::wayland::shell::xdg::SurfaceCachedState>();
            let cached = guard.current();
            (cached.max_size.w, cached.max_size.h)
        });
        return (
            (max_w > 0).then_some(max_w),
            (max_h > 0).then_some(max_h),
        );
    }
    (None, None)
}

/// Checks whether a Wayland `xdg_toplevel` or X11 window is a transient dialog, modal,
/// Picture-in-Picture, file picker, or fixed-size utility window that should float automatically.
pub fn should_auto_float(window: &Window) -> bool {
    if let Some(x11) = window.x11_surface() {
        if x11.is_popup() || x11.is_transient_for().is_some() {
            return true;
        }
        if let Some(hints) = x11.size_hints() {
            if let (Some(min), Some(max)) = (hints.min_size, hints.max_size) {
                if min.0 > 0 && min.1 > 0 && min == max {
                    return true;
                }
            }
        }
    }
    if let Some(toplevel) = window.toplevel() {
        let has_parent_or_modal = with_states(toplevel.wl_surface(), |states| {
            let role = states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .and_then(|data| data.lock().ok());
            if let Some(r) = role.as_ref() {
                if r.parent.is_some() || r.modal {
                    return true;
                }
            }
            false
        });
        if has_parent_or_modal {
            return true;
        }
    }

    let (app_id, title) = window_metadata(window);
    let app_lower = app_id.to_ascii_lowercase();
    let title_lower = title.to_ascii_lowercase();
    for float_app in [
        "pavucontrol",
        "nm-connection-editor",
        "blueman-manager",
        "xdg-desktop-portal",
        "pinentry",
        "polkit",
        "corten-welcome",
        "calculator",
        "qalculate",
        "gnome-calculator",
        "kcalc",
        "file-roller",
        "ark",
        "imv",
        "feh",
        "color-picker",
        "eyedropper",
    ] {
        if app_lower.contains(float_app) {
            return true;
        }
    }
    for float_title in [
        "picture-in-picture",
        "picture in picture",
        "resim içinde resim",
        "open file",
        "save file",
        "dosya aç",
        "dosya kaydet",
        "authentication required",
        "hesap makinesi",
        "preferences",
        "ayarlar",
        "about",
        "hakkında",
    ] {
        if title_lower.contains(float_title) {
            return true;
        }
    }
    false
}

#[derive(Debug, Clone, Default)]
pub struct WindowRuleMatch {
    pub floating: Option<bool>,
    pub workspace: Option<usize>,
    pub fullscreen: Option<bool>,
    pub opacity: Option<f32>,
    pub blur: Option<bool>,
    pub sticky: Option<bool>,
    pub scratchpad: Option<bool>,
    pub decoration_mode: Option<String>,
}

/// Evaluates user-configured `window_rules` against a window's `(app_id, title)`
/// and applies smart workspace auto-routing for well-known app classes.
pub fn evaluate_window_rules_full(
    window: &Window,
    rules: &[WindowRuleConfig],
) -> WindowRuleMatch {
    let (app_id, title) = window_metadata(window);
    let app_lower = app_id.to_ascii_lowercase();
    let title_lower = title.to_ascii_lowercase();

    let mut m = WindowRuleMatch::default();

    for rule in rules {
        let app_matches = rule.app_id.as_ref().is_none_or(|pattern| {
            !pattern.is_empty() && app_lower.contains(&pattern.to_ascii_lowercase())
        });
        let title_matches = rule.title.as_ref().is_none_or(|pattern| {
            !pattern.is_empty() && title_lower.contains(&pattern.to_ascii_lowercase())
        });
        if (rule.app_id.is_some() || rule.title.is_some()) && app_matches && title_matches {
            if let Some(f) = rule.floating {
                m.floating = Some(f);
            }
            if let Some(ws) = rule.workspace {
                m.workspace = Some(ws.clamp(1, 9));
            }
            if let Some(fs) = rule.fullscreen {
                m.fullscreen = Some(fs);
            }
            if let Some(op) = rule.opacity_float() {
                m.opacity = Some(op);
            }
            if let Some(b) = rule.blur {
                m.blur = Some(b);
            }
            if let Some(st) = rule.sticky {
                m.sticky = Some(st);
            }
            if let Some(sp) = rule.scratchpad {
                m.scratchpad = Some(sp);
            }
            if let Some(ref dm) = rule.decoration_mode {
                m.decoration_mode = Some(dm.clone());
            }
        }
    }

    // Smart default workspace auto-routing (unless explicitly configured by user)
    if m.workspace.is_none() {
        for (pattern, target_ws) in [
            // Workspace 1: Code / Dev (💻 Kod)
            ("code", 1), ("cursor", 1), ("zed", 1), ("sublime_text", 1), ("vscodium", 1),
            // Workspace 2: Web Browsers (🌐 Web)
            ("firefox", 2), ("chromium", 2), ("chrome", 2), ("brave", 2), ("zen", 2), ("epiphany", 2),
            // Workspace 3: Chat & Communication (💬 Sohbet)
            ("discord", 3), ("telegram", 3), ("slack", 3), ("signal", 3), ("vesktop", 3),
            // Workspace 4: Media Players (🎵 Medya)
            ("spotify", 4), ("rhythmbox", 4), ("vlc", 4), ("audacious", 4), ("amberol", 4),
        ] {
            if app_lower.contains(pattern) {
                m.workspace = Some(target_ws);
                break;
            }
        }
    }

    m
}

pub fn evaluate_window_rules(
    window: &Window,
    rules: &[WindowRuleConfig],
) -> (Option<bool>, Option<usize>, Option<bool>) {
    let m = evaluate_window_rules_full(window, rules);
    (m.floating, m.workspace, m.fullscreen)
}

pub fn is_window_blur_eligible(window: &Window, rules: &[WindowRuleConfig]) -> bool {
    let (app_id, _) = window_metadata(window);
    let app_lower = app_id.to_ascii_lowercase();
    let m = evaluate_window_rules_full(window, rules);
    if m.blur == Some(true) {
        return true;
    }
    if m.opacity.is_some_and(|o| o < 0.99) {
        return true;
    }
    for term in [
        "alacritty", "kitty", "foot", "wezterm", "gnome-terminal", "tilix", "urxvt", "xterm",
        "neovide", "corten-terminal", "terminal",
    ] {
        if app_lower.contains(term) {
            return true;
        }
    }
    false
}

/// Direction for binary space partitioning tree splits
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SplitDirection {
    #[default]
    Horizontal, // Split left/right
    Vertical,   // Split top/bottom
}

impl SplitDirection {
    pub fn toggle(self) -> Self {
        match self {
            Self::Horizontal => Self::Vertical,
            Self::Vertical => Self::Horizontal,
        }
    }
}

/// Binary Space Partitioning tree node representing tiling structure
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum BspNode {
    Leaf(u64),
    Split {
        dir: SplitDirection,
        ratio: f32,
        left: Box<BspNode>,
        right: Box<BspNode>,
    },
}

impl BspNode {
    #[allow(dead_code)]
    pub fn leaf(window_id: u64) -> Self {
        Self::Leaf(window_id)
    }

    pub fn contains(&self, window_id: u64) -> bool {
        match self {
            Self::Leaf(id) => *id == window_id,
            Self::Split { left, right, .. } => left.contains(window_id) || right.contains(window_id),
        }
    }

    pub fn is_leaf_id(&self, target_id: u64) -> bool {
        match self {
            Self::Leaf(id) => *id == target_id,
            _ => false,
        }
    }

    pub fn remove(&mut self, window_id: u64) {
        if let Self::Split { left, right, .. } = self {
            if left.is_leaf_id(window_id) {
                *self = (**right).clone();
            } else if right.is_leaf_id(window_id) {
                *self = (**left).clone();
            } else {
                if left.contains(window_id) {
                    left.remove(window_id);
                } else if right.contains(window_id) {
                    right.remove(window_id);
                }
            }
        }
    }

    pub fn insert_adjacent(
        &mut self,
        target_id: u64,
        new_id: u64,
        dir: SplitDirection,
        new_on_left_or_top: bool,
    ) -> bool {
        match self {
            Self::Leaf(id) => {
                if *id == target_id {
                    let target_node = Box::new(Self::Leaf(target_id));
                    let new_node = Box::new(Self::Leaf(new_id));
                    let (left, right) = if new_on_left_or_top {
                        (new_node, target_node)
                    } else {
                        (target_node, new_node)
                    };
                    *self = Self::Split {
                        dir,
                        ratio: 0.5,
                        left,
                        right,
                    };
                    true
                } else {
                    false
                }
            }
            Self::Split { left, right, .. } => {
                if left.insert_adjacent(target_id, new_id, dir, new_on_left_or_top) {
                    true
                } else {
                    right.insert_adjacent(target_id, new_id, dir, new_on_left_or_top)
                }
            }
        }
    }

    pub fn insert_default(&mut self, new_id: u64, dir: SplitDirection) {
        match self {
            Self::Leaf(id) => {
                let old_id = *id;
                *self = Self::Split {
                    dir,
                    ratio: 0.5,
                    left: Box::new(Self::Leaf(old_id)),
                    right: Box::new(Self::Leaf(new_id)),
                };
            }
            Self::Split { right, .. } => {
                right.insert_default(new_id, dir.toggle());
            }
        }
    }

    pub fn compute_rects(
        &self,
        rect: Rectangle<i32, Logical>,
        gap: i32,
        out: &mut HashMap<u64, Rectangle<i32, Logical>>,
    ) {
        match self {
            Self::Leaf(id) => {
                out.insert(*id, rect);
            }
            Self::Split { dir, ratio, left, right } => {
                let clamped_ratio = ratio.clamp(0.1, 0.9);
                match dir {
                    SplitDirection::Horizontal => {
                        let total_w = (rect.size.w - gap).max(20);
                        let w_left = ((total_w as f32) * clamped_ratio).round() as i32;
                        let w_right = (total_w - w_left).max(10);
                        let left_rect = Rectangle::new(rect.loc, Size::from((w_left, rect.size.h)));
                        let right_rect = Rectangle::new(
                            Point::from((rect.loc.x + w_left + gap, rect.loc.y)),
                            Size::from((w_right, rect.size.h)),
                        );
                        left.compute_rects(left_rect, gap, out);
                        right.compute_rects(right_rect, gap, out);
                    }
                    SplitDirection::Vertical => {
                        let total_h = (rect.size.h - gap).max(20);
                        let h_top = ((total_h as f32) * clamped_ratio).round() as i32;
                        let h_bottom = (total_h - h_top).max(10);
                        let top_rect = Rectangle::new(rect.loc, Size::from((rect.size.w, h_top)));
                        let bottom_rect = Rectangle::new(
                            Point::from((rect.loc.x, rect.loc.y + h_top + gap)),
                            Size::from((rect.size.w, h_bottom)),
                        );
                        left.compute_rects(top_rect, gap, out);
                        right.compute_rects(bottom_rect, gap, out);
                    }
                }
            }
        }
    }

    pub fn collect_stale_leaves(&self, active_ids: &[u64], out: &mut Vec<u64>) {
        match self {
            Self::Leaf(id) => {
                if !active_ids.contains(id) {
                    out.push(*id);
                }
            }
            Self::Split { left, right, .. } => {
                left.collect_stale_leaves(active_ids, out);
                right.collect_stale_leaves(active_ids, out);
            }
        }
    }
}

/// Dynamic Tiling Layout Manager for Corten Compositor
///
/// Implements Master-Stack tiling algorithm with configurable gaps,
/// responsive proportions, and smooth Spring physics / animated transitions.
#[derive(Debug)]
pub struct LayoutManager {
    windows: Vec<Window>,
    master_ratio: f32, // Ratio for master window (e.g., 0.55 = 55% width)
    gap: i32,          // Outer & Inner margin gap between windows in pixels
    mode: LayoutMode,
    active_index: usize,
    /// Cached animated window geometries keyed by stable window ID
    animated_rects: HashMap<u64, Rectangle<f32, Logical>>,
    /// Cached animated window opacity (0.15 -> 1.0 spawn fade-in) keyed by stable window ID
    animated_alphas: HashMap<u64, f32>,
    /// Configured or default target opacity per window
    target_alphas: HashMap<u64, f32>,
    /// Tree representation for manual BSP tiling
    bsp_tree: Option<BspNode>,
    /// Current selected split direction for BSP tiling (Super+B / Super+V)
    split_direction: SplitDirection,
    /// Windows that are floating (not tiled in master-stack)
    floating_windows: Vec<Window>,
    /// Saved floating geometries (location & size) keyed by stable window ID
    floating_geometries: HashMap<u64, Rectangle<i32, Logical>>,
    /// Windows that are currently fullscreen keyed by stable window ID
    fullscreen_windows: HashSet<u64>,
    /// Windows that are currently maximized (filling usable work area with decorations)
    maximized_windows: HashSet<u64>,
    /// Saved floating geometries before maximization to allow restoring
    saved_unmaximized_geometries: HashMap<u64, Rectangle<i32, Logical>>,
    /// Windows whose initial rules/auto-float have already been evaluated after metadata commit
    evaluated_windows: HashSet<u64>,
    workspace: usize,
    workspace_windows: HashMap<usize, Vec<Window>>,
    workspace_floating: HashMap<usize, Vec<Window>>,
    active_floating: Option<Window>,
    /// Horizontal parallax slide offset (in logical px) during workspace transitions
    workspace_slide_offset: f32,
    /// Tabbed window groups (each group contains >= 2 stable window IDs sharing one frame)
    tab_groups: Vec<Vec<u64>>,
    /// Per-workspace layout mode overrides (e.g. WS 1 Floating, WS 2 BSP, WS 3 Scrollable)
    workspace_modes: HashMap<usize, LayoutMode>,
    /// Sticky / Pinned window IDs (e.g. Picture-in-Picture) that follow across all workspaces
    sticky_windows: HashSet<u64>,
    /// Hidden drop-down scratchpad window pool
    scratchpad_windows: Vec<Window>,
    /// Horizontal scroll offset (in logical px) for `LayoutMode::Scrollable` (Niri strip)
    scroll_offset_x: f64,
    /// Target horizontal scroll offset for smooth spring-damped strip transitions
    target_scroll_offset_x: f64,
    /// Column width fraction (0.25..=1.0) for `LayoutMode::Scrollable`
    scroll_column_fraction: f32,
    /// One-shot session restore queue loaded once at startup (consumed as each saved window spawns)
    pending_session_entries: Vec<crate::state::SavedWindowEntry>,
}

impl Default for LayoutManager {
    fn default() -> Self {
        Self::new()
    }
}

impl LayoutManager {
    pub fn new() -> Self {
        Self {
            windows: Vec::new(),
            master_ratio: 0.55,
            gap: 12, // 12px Corten design spacing
            mode: LayoutMode::Floating,
            active_index: 0,
            animated_rects: HashMap::new(),
            animated_alphas: HashMap::new(),
            target_alphas: HashMap::new(),
            floating_windows: Vec::new(),
            floating_geometries: HashMap::new(),
            fullscreen_windows: HashSet::new(),
            maximized_windows: HashSet::new(),
            saved_unmaximized_geometries: HashMap::new(),
            evaluated_windows: HashSet::new(),
            workspace: 1,
            workspace_windows: HashMap::new(),
            workspace_floating: HashMap::new(),
            active_floating: None,
            workspace_slide_offset: 0.0,
            tab_groups: Vec::new(),
            workspace_modes: HashMap::new(),
            sticky_windows: HashSet::new(),
            scratchpad_windows: Vec::new(),
            scroll_offset_x: 0.0,
            target_scroll_offset_x: 0.0,
            scroll_column_fraction: 0.58,
            pending_session_entries: crate::state::load_window_session_from_disk(),
            bsp_tree: None,
            split_direction: SplitDirection::Horizontal,
        }
    }

    #[allow(dead_code)]
    pub fn split_direction(&self) -> SplitDirection {
        self.split_direction
    }

    pub fn set_split_direction(&mut self, dir: SplitDirection) {
        self.split_direction = dir;
    }

    #[allow(dead_code)]
    pub fn toggle_split_direction(&mut self) -> SplitDirection {
        self.split_direction = self.split_direction.toggle();
        self.split_direction
    }

    #[allow(dead_code)]
    pub fn bsp_tree(&self) -> Option<&BspNode> {
        self.bsp_tree.as_ref()
    }

    #[allow(dead_code)]
    pub fn mode(&self) -> LayoutMode {
        self.workspace_modes
            .get(&self.workspace)
            .copied()
            .unwrap_or(self.mode)
    }

    pub fn set_mode(&mut self, mode: LayoutMode) {
        self.mode = mode;
        self.workspace_modes.insert(self.workspace, mode);
    }

    pub fn set_workspace_mode(&mut self, ws: usize, mode: LayoutMode) {
        let ws = ws.clamp(1, 9);
        self.workspace_modes.insert(ws, mode);
        if ws == self.workspace {
            self.mode = mode;
        }
    }

    pub fn cycle_mode(&mut self) -> LayoutMode {
        let next = self.mode().next();
        self.set_mode(next);
        next
    }

    /// Toggles sticky/pinned mode (PiP window follows across all workspaces).
    pub fn toggle_sticky(&mut self, window: &Window) -> bool {
        let wid = window_stable_id(window);
        if self.sticky_windows.contains(&wid) {
            self.sticky_windows.remove(&wid);
            false
        } else {
            self.sticky_windows.insert(wid);
            true
        }
    }

    /// Returns true if `window` is pinned across all workspaces.
    #[allow(dead_code)]
    pub fn is_sticky(&self, window: &Window) -> bool {
        self.sticky_windows.contains(&window_stable_id(window))
    }

    /// Moves a window into the hidden drop-down scratchpad pool.
    pub fn move_to_scratchpad(&mut self, window: &Window) {
        self.remove_window(window);
        if !self.scratchpad_windows.iter().any(|w| w == window) {
            self.scratchpad_windows.push(window.clone());
        }
    }

    /// Toggles the drop-down scratchpad window: if a scratchpad window is currently active/visible
    /// in floating windows, hides it back into the pool; otherwise pops the top scratchpad window
    /// centered at 72% x 68% of `usable_area`.
    pub fn toggle_scratchpad(
        &mut self,
        usable_area: Rectangle<i32, Logical>,
    ) -> Option<Window> {
        // If we have a scratchpad window waiting, bring it up centered as floating
        if let Some(win) = self.scratchpad_windows.pop() {
            let w = ((usable_area.size.w as f32) * 0.72).round() as i32;
            let h = ((usable_area.size.h as f32) * 0.68).round() as i32;
            let x = usable_area.loc.x + (usable_area.size.w - w) / 2;
            let y = usable_area.loc.y + (usable_area.size.h - h) / 2;
            let rect = Rectangle::new(Point::from((x, y)), Size::from((w.max(320), h.max(240))));
            self.floating_windows.push(win.clone());
            self.floating_geometries.insert(window_stable_id(&win), rect);
            self.active_floating = Some(win.clone());
            configure_window_geometry(&win, rect);
            return Some(win);
        }
        // Otherwise hide the currently active window into the scratchpad pool
        if let Some(active) = self.active_window().cloned() {
            self.move_to_scratchpad(&active);
        }
        None
    }

    /// Scrolls the Niri-style horizontal strip by `delta_px`.
    #[allow(dead_code)]
    pub fn scroll_strip(&mut self, delta_px: f64) {
        self.scroll_offset_x = (self.scroll_offset_x + delta_px).max(0.0);
    }

    /// Steps the Niri-style horizontal strip left (`dir = -1`) or right (`dir = 1`) by one column.
    pub fn scroll_column_step(&mut self, dir: i32, usable_area: Rectangle<i32, Logical>) {
        let col_w = ((usable_area.size.w as f64) * self.scroll_column_fraction as f64).max(320.0);
        let step = col_w + self.gap as f64;
        let max_scroll = (self.windows.len().saturating_sub(1) as f64 * step).max(0.0);
        self.target_scroll_offset_x = (self.target_scroll_offset_x + (dir as f64) * step).clamp(0.0, max_scroll);
        self.scroll_offset_x = crate::render::step_smooth_scroll(self.scroll_offset_x, self.target_scroll_offset_x);
    }

    /// Advances smooth horizontal scroll strip animation by one frame.
    pub fn step_scroll_animation(&mut self) {
        self.scroll_offset_x = crate::render::step_smooth_scroll(self.scroll_offset_x, self.target_scroll_offset_x);
    }

    /// Cycles column width preset in `LayoutMode::Scrollable`: 33% -> 50% -> 66% -> 100%.
    pub fn cycle_scroll_column_fraction(&mut self) -> f32 {
        self.scroll_column_fraction = if self.scroll_column_fraction < 0.40 {
            0.50
        } else if self.scroll_column_fraction < 0.60 {
            0.66
        } else if self.scroll_column_fraction < 0.85 {
            1.00
        } else {
            0.33
        };
        self.scroll_column_fraction
    }

    /// Dynamic workspace count (GNOME/Niri style): returns `highest_occupied_ws + 1` clamped to `1..=9`.
    #[allow(dead_code)]
    pub fn dynamic_workspace_count(&self) -> usize {
        let mut highest = self.workspace;
        for (&ws, wins) in &self.workspace_windows {
            if !wins.is_empty() {
                highest = highest.max(ws);
            }
        }
        for (&ws, wins) in &self.workspace_floating {
            if !wins.is_empty() {
                highest = highest.max(ws);
            }
        }
        if !self.windows.is_empty() || !self.floating_windows.is_empty() {
            highest = highest.max(self.workspace);
        }
        (highest + 1).clamp(1, 9)
    }

    /// Set margin gap in pixels
    pub fn set_gap(&mut self, gap: i32) {
        self.gap = gap.max(0);
    }

    /// Get current margin gap
    #[allow(dead_code)]
    pub fn gap(&self) -> i32 {
        self.gap
    }

    /// Check if a window is currently floating
    #[allow(dead_code)]
    pub fn is_floating(&self, window: &Window) -> bool {
        self.floating_windows.iter().any(|w| w == window)
    }

    /// Toggle floating state of the active window or a specific window
    pub fn toggle_floating(&mut self, window: Window) -> bool {
        if let Some(pos) = self.floating_windows.iter().position(|w| w == &window) {
            // Un-float: return to tiled layout
            self.floating_windows.remove(pos);
            self.add_window(window);
            false
        } else {
            // Float: remove from tiled layout and add to floating
            self.remove_window(&window);
            self.floating_windows.push(window);
            true
        }
    }

    /// Explicitly set floating state of a window
    pub fn set_floating(&mut self, window: &Window, floating: bool) {
        if floating {
            if !self.is_floating(window) {
                let win_id = window_stable_id(window);
                if let Some(pos) = self.windows.iter().position(|w| w == window) {
                    let win = self.windows.remove(pos);
                    self.animated_rects.remove(&win_id);
                    if self.active_index >= self.windows.len() && !self.windows.is_empty() {
                        self.active_index = self.windows.len() - 1;
                    }
                    self.floating_windows.push(win.clone());
                    self.active_floating = Some(win);
                }
                if let Some(ref mut tree) = self.bsp_tree {
                    if tree.is_leaf_id(win_id) {
                        self.bsp_tree = None;
                    } else {
                        tree.remove(win_id);
                    }
                }
            }
        } else if self.is_floating(window) {
            self.floating_windows.retain(|w| w != window);
            if self.active_floating.as_ref() == Some(window) {
                self.active_floating = self.floating_windows.last().cloned();
            }
            self.add_window(window.clone());
        }
    }

    /// Synchronizes the BSP tree with the currently tiled windows
    pub fn sync_bsp_tree(&mut self) {
        let active_ids: Vec<u64> = self.windows.iter().map(window_stable_id).collect();

        if active_ids.is_empty() {
            self.bsp_tree = None;
            return;
        }

        if let Some(ref mut tree) = self.bsp_tree {
            let mut to_remove = Vec::new();
            tree.collect_stale_leaves(&active_ids, &mut to_remove);
            for stale_id in to_remove {
                if tree.contains(stale_id) {
                    if tree.is_leaf_id(stale_id) {
                        self.bsp_tree = None;
                        break;
                    } else {
                        tree.remove(stale_id);
                    }
                }
            }
        }

        if self.bsp_tree.is_none() && !active_ids.is_empty() {
            self.bsp_tree = Some(BspNode::Leaf(active_ids[0]));
        }

        if let Some(ref mut tree) = self.bsp_tree {
            for &id in &active_ids {
                if !tree.contains(id) {
                    tree.insert_default(id, self.split_direction);
                }
            }
        }
    }

    /// Attaches or splits a dragged window adjacent to a target window in BSP tree
    pub fn drag_split_window(
        &mut self,
        dragged: Window,
        target: Window,
        dir: SplitDirection,
        new_on_left_or_top: bool,
    ) {
        let dragged_id = window_stable_id(&dragged);
        let target_id = window_stable_id(&target);
        self.floating_windows.retain(|w| window_stable_id(w) != dragged_id);
        if !self.windows.iter().any(|w| window_stable_id(w) == dragged_id) {
            self.windows.push(dragged.clone());
        }
        self.sync_bsp_tree();

        if let Some(ref mut tree) = self.bsp_tree {
            if tree.contains(dragged_id) {
                if tree.is_leaf_id(dragged_id) {
                    *tree = BspNode::Split {
                        dir,
                        ratio: 0.5,
                        left: Box::new(if new_on_left_or_top { BspNode::Leaf(dragged_id) } else { BspNode::Leaf(target_id) }),
                        right: Box::new(if new_on_left_or_top { BspNode::Leaf(target_id) } else { BspNode::Leaf(dragged_id) }),
                    };
                } else {
                    tree.remove(dragged_id);
                    if !tree.insert_adjacent(target_id, dragged_id, dir, new_on_left_or_top) {
                        tree.insert_default(dragged_id, dir);
                    }
                }
            } else if !tree.insert_adjacent(target_id, dragged_id, dir, new_on_left_or_top) {
                tree.insert_default(dragged_id, dir);
            }
        } else {
            self.bsp_tree = Some(BspNode::Leaf(dragged_id));
        }

        // Reorder self.windows so that dragged is placed right next to target
        if let Some(dragged_pos) = self.windows.iter().position(|w| window_stable_id(w) == dragged_id) {
            let win = self.windows.remove(dragged_pos);
            let insert_idx = if let Some(target_pos) = self.windows.iter().position(|w| window_stable_id(w) == target_id) {
                if new_on_left_or_top { target_pos } else { target_pos + 1 }
            } else {
                self.windows.len()
            };
            self.windows.insert(insert_idx.min(self.windows.len()), win);
        }
        self.active_index = self.windows.iter().position(|w| window_stable_id(w) == dragged_id).unwrap_or(0);
    }

    /// Add a new window into the layout (auto-detect dialogs as floating)
    pub fn add_window(&mut self, window: Window) {
        if !self.windows.iter().any(|w| w == &window)
            && !self.floating_windows.iter().any(|w| w == &window)
        {
            if self.mode == LayoutMode::Floating {
                self.floating_windows.push(window.clone());
                self.active_floating = Some(window);
            } else {
                self.windows.push(window);
                self.active_index = self.windows.len().saturating_sub(1);
            }
        }
    }

    /// Add a new floating window into the layout
    pub fn add_floating_window(&mut self, window: Window) {
        if !self.windows.iter().any(|w| w == &window)
            && !self.floating_windows.iter().any(|w| w == &window)
        {
            self.floating_windows.push(window.clone());
            self.active_floating = Some(window);
        }
    }

    /// Remove a window from the layout
    pub fn remove_window(&mut self, window: &Window) {
        let win_id = window_stable_id(window);
        self.animated_rects.remove(&win_id);
        self.animated_alphas.remove(&win_id);
        self.target_alphas.remove(&win_id);
        self.floating_geometries.remove(&win_id);
        self.fullscreen_windows.remove(&win_id);
        self.maximized_windows.remove(&win_id);
        self.saved_unmaximized_geometries.remove(&win_id);
        self.evaluated_windows.remove(&win_id);
        if let Some(ref mut tree) = self.bsp_tree {
            if tree.is_leaf_id(win_id) {
                self.bsp_tree = None;
            } else {
                tree.remove(win_id);
            }
        }
        if let Some(pos) = self.windows.iter().position(|w| w == window) {
            self.windows.remove(pos);
            if self.active_index >= self.windows.len() && !self.windows.is_empty() {
                self.active_index = self.windows.len() - 1;
            }
        }
        self.floating_windows.retain(|w| w != window);
        if self.active_floating.as_ref() == Some(window) {
            self.active_floating = self.floating_windows.last().cloned();
        }
        for windows in self.workspace_windows.values_mut() {
            windows.retain(|candidate| candidate != window);
        }
        for windows in self.workspace_floating.values_mut() {
            windows.retain(|candidate| candidate != window);
        }
    }

    /// Evaluates auto-float heuristics and configured `window_rules` once a window's initial metadata is ready.
    pub fn evaluate_window_on_commit(
        &mut self,
        window: &Window,
        rules: &[WindowRuleConfig],
    ) -> bool {
        let wid = window_stable_id(window);
        if self.evaluated_windows.contains(&wid) {
            return false;
        }
        let (app_id, title) = window_metadata(window);
        if title == "winit window" {
            return false;
        }
        let auto_float = should_auto_float(window);
        if app_id.is_empty() && title.is_empty() && !auto_float {
            return false;
        }
        self.evaluated_windows.insert(wid);

        let mut rule_match = evaluate_window_rules_full(window, rules);
        let mut session_geo: Option<Rectangle<i32, Logical>> = None;
        if !app_id.is_empty() {
            if let Some(pos) = self
                .pending_session_entries
                .iter()
                .position(|e| e.app_id == app_id)
            {
                let saved = self.pending_session_entries.remove(pos);
                if rule_match.workspace.is_none() && (1..=9).contains(&saved.workspace) {
                    rule_match.workspace = Some(saved.workspace);
                }
                if rule_match.floating.is_none() && saved.is_floating {
                    rule_match.floating = Some(true);
                    session_geo = Some(Rectangle::new(
                        Point::from((saved.x, saved.y)),
                        Size::from((saved.width.max(160), saved.height.max(120))),
                    ));
                }
            }
        }
        let target_float = rule_match.floating.unwrap_or(auto_float);
        let mut changed = false;

        if let Some(op) = rule_match.opacity {
            self.set_window_target_alpha(window, op);
            changed = true;
        }

        if let Some(fs) = rule_match.fullscreen {
            self.set_fullscreen(window, fs);
            changed = true;
        }

        if let Some(sticky) = rule_match.sticky {
            if sticky {
                self.sticky_windows.insert(wid);
            } else {
                self.sticky_windows.remove(&wid);
            }
            changed = true;
        }

        if let Some(ref dec_mode) = rule_match.decoration_mode {
            if let Some(toplevel) = window.toplevel() {
                toplevel.with_pending_state(|tstate| {
                    use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode;
                    tstate.decoration_mode = Some(if dec_mode.eq_ignore_ascii_case("csd") {
                        Mode::ClientSide
                    } else {
                        Mode::ServerSide
                    });
                });
                toplevel.send_configure();
            }
        }

        if target_float && !self.is_floating(window) {
            if let Some(pos) = self.windows.iter().position(|w| w == window) {
                let win = self.windows.remove(pos);
                self.animated_rects.remove(&wid);
                if self.active_index >= self.windows.len() && !self.windows.is_empty() {
                    self.active_index = self.windows.len() - 1;
                }
                self.floating_windows.push(win.clone());
                self.active_floating = Some(win);
                changed = true;
            }
        }

        if let Some(geo) = session_geo {
            self.set_floating_geometry(window, geo);
            configure_window_geometry(window, geo);
            changed = true;
        }

        if rule_match.scratchpad == Some(true) {
            self.move_to_scratchpad(window);
            return true;
        }

        if let Some(target_ws) = rule_match.workspace {
            let target_ws = target_ws.clamp(1, 9);
            if target_ws != self.workspace {
                if let Some(pos) = self.windows.iter().position(|w| w == window) {
                    let win = self.windows.remove(pos);
                    self.animated_rects.remove(&wid);
                    if self.active_index >= self.windows.len() && !self.windows.is_empty() {
                        self.active_index = self.windows.len() - 1;
                    }
                    self.workspace_windows
                        .entry(target_ws)
                        .or_default()
                        .push(win);
                    changed = true;
                } else if let Some(pos) = self.floating_windows.iter().position(|w| w == window) {
                    let win = self.floating_windows.remove(pos);
                    self.workspace_floating
                        .entry(target_ws)
                        .or_default()
                        .push(win);
                    changed = true;
                }
            }
        }

        changed
    }

    pub fn switch_workspace(&mut self, workspace: usize) {
        let workspace = workspace.clamp(1, 9);
        if workspace == self.workspace {
            return;
        }
        let old = self.workspace;
        let mut current = std::mem::take(&mut self.windows);
        let mut floating = std::mem::take(&mut self.floating_windows);

        // Extract sticky (pinned PiP) windows so they follow the user to the new workspace
        let mut sticky_tiled = Vec::new();
        current.retain(|w| {
            if self.sticky_windows.contains(&window_stable_id(w)) {
                sticky_tiled.push(w.clone());
                false
            } else {
                true
            }
        });
        let mut sticky_float = Vec::new();
        floating.retain(|w| {
            if self.sticky_windows.contains(&window_stable_id(w)) {
                sticky_float.push(w.clone());
                false
            } else {
                true
            }
        });

        self.workspace_windows.insert(old, current);
        self.workspace_floating.insert(old, floating);
        self.windows = self
            .workspace_windows
            .remove(&workspace)
            .unwrap_or_default();
        self.floating_windows = self
            .workspace_floating
            .remove(&workspace)
            .unwrap_or_default();

        for w in sticky_tiled {
            if !self.windows.iter().any(|existing| existing == &w) {
                self.windows.push(w);
            }
        }
        for w in sticky_float {
            if !self.floating_windows.iter().any(|existing| existing == &w) {
                self.floating_windows.push(w);
            }
        }

        self.workspace = workspace;
        self.active_index = self.active_index.min(self.windows.len().saturating_sub(1));
        self.active_floating = None;
        self.animated_rects.clear();
        let slide_dir = if workspace > old { 1.0_f32 } else { -1.0_f32 };
        self.workspace_slide_offset = slide_dir * 160.0;
    }

    /// Return count of managed windows
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.windows.len()
    }

    /// Check if no windows are managed
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.windows.is_empty()
    }

    /// Return slice of all windows
    pub fn windows(&self) -> &[Window] {
        &self.windows
    }

    pub fn contains_window(&self, window: &Window) -> bool {
        self.windows.iter().any(|candidate| candidate == window)
            || self
                .floating_windows
                .iter()
                .any(|candidate| candidate == window)
    }

    /// Extracts and returns all windows across all workspaces in this layout manager along with saved floating geometries
    #[allow(clippy::type_complexity)]
    pub fn drain_all_windows(
        &mut self,
    ) -> Vec<(usize, Window, bool, Option<Rectangle<i32, Logical>>)> {
        let mut all = Vec::new();
        // Current workspace tiled windows
        for w in self.windows.drain(..) {
            all.push((self.workspace, w, false, None));
        }
        // Current workspace floating windows
        for w in self.floating_windows.drain(..) {
            let wid = window_stable_id(&w);
            let geo = self.floating_geometries.remove(&wid);
            all.push((self.workspace, w, true, geo));
        }
        // Other workspaces
        for (ws, list) in self.workspace_windows.drain() {
            for w in list {
                all.push((ws, w, false, None));
            }
        }
        for (ws, list) in self.workspace_floating.drain() {
            for w in list {
                let wid = window_stable_id(&w);
                let geo = self.floating_geometries.remove(&wid);
                all.push((ws, w, true, geo));
            }
        }
        self.floating_geometries.clear();
        self.animated_rects.clear();
        self.active_index = 0;
        all
    }

    /// Restores an orphaned window into this layout manager at specified workspace, preserving its floating geometry if present
    pub fn rescue_window(
        &mut self,
        workspace: usize,
        window: Window,
        is_floating: bool,
        saved_geo: Option<Rectangle<i32, Logical>>,
    ) {
        let ws = workspace.clamp(1, 9);
        let win_id = window_stable_id(&window);
        if let Some(geo) = saved_geo {
            self.floating_geometries.insert(win_id, geo);
        }
        if ws == self.workspace {
            if is_floating {
                if !self.floating_windows.contains(&window) {
                    self.floating_windows.push(window);
                }
            } else if !self.windows.contains(&window) {
                self.windows.push(window);
            }
        } else if is_floating {
            let list = self.workspace_floating.entry(ws).or_default();
            if !list.contains(&window) {
                list.push(window);
            }
        } else {
            let list = self.workspace_windows.entry(ws).or_default();
            if !list.contains(&window) {
                list.push(window);
            }
        }
    }

    /// Clamps all saved floating geometries to remain within the visible usable area.
    /// Prevents windows from being stranded off-screen when an output is disconnected or scaled.
    pub fn clamp_floating_geometries_to_area(&mut self, usable_area: Rectangle<i32, Logical>) {
        if usable_area.size.w <= 0 || usable_area.size.h <= 0 {
            return;
        }

        for (win_id, rect) in self.floating_geometries.iter_mut() {
            // 1. Constrain size so it doesn't exceed screen dimensions
            rect.size.w = rect.size.w.min(usable_area.size.w - 32).max(200);
            rect.size.h = rect.size.h.min(usable_area.size.h - 32).max(140);

            // 2. Constrain location X and Y within visible bounds
            let min_x = usable_area.loc.x;
            let max_x = (usable_area.loc.x + usable_area.size.w - rect.size.w).max(min_x);
            let min_y = usable_area.loc.y;
            let max_y = (usable_area.loc.y + usable_area.size.h - rect.size.h).max(min_y);

            rect.loc.x = rect.loc.x.clamp(min_x, max_x);
            rect.loc.y = rect.loc.y.clamp(min_y, max_y);

            // Synchronize animation rect so windows don't jump across disconnected monitor coordinates
            self.animated_rects.insert(
                *win_id,
                Rectangle::new(
                    Point::from((rect.loc.x as f32, rect.loc.y as f32)),
                    Size::from((rect.size.w as f32, rect.size.h as f32)),
                ),
            );
        }
    }

    /// Currently active window (either floating top or tiled active)
    pub fn active_window(&self) -> Option<&Window> {
        if let Some(ref fw) = self.active_floating {
            if self.floating_windows.contains(fw) {
                return Some(fw);
            }
        }
        self.windows.get(self.active_index)
    }

    /// Set active window by instance and raise it to top of Z-order
    pub fn set_active_window(&mut self, window: &Window) {
        if let Some(pos) = self.windows.iter().position(|w| w == window) {
            self.active_index = pos;
            self.active_floating = None;
        } else if let Some(pos) = self.floating_windows.iter().position(|w| w == window) {
            // Bring clicked floating window to the top of Z-order!
            let win = self.floating_windows.remove(pos);
            self.floating_windows.push(win.clone());
            self.active_floating = Some(win);
        }
    }

    /// Switch active window to next in cycle (cycles through all tiled and floating windows)
    pub fn next_window(&mut self) -> Option<&Window> {
        let total = self.windows.len() + self.floating_windows.len();
        if total == 0 {
            return None;
        }

        let current_active = self.active_window().cloned();
        let current_pos = current_active.as_ref().and_then(|act| {
            if let Some(pos) = self.windows.iter().position(|w| w == act) {
                Some(pos)
            } else {
                self.floating_windows.iter().position(|w| w == act).map(|p| self.windows.len() + p)
            }
        }).unwrap_or(0);

        let next_pos = (current_pos + 1) % total;
        let target = if next_pos < self.windows.len() {
            self.windows[next_pos].clone()
        } else {
            self.floating_windows[next_pos - self.windows.len()].clone()
        };

        if let Some(pos) = self.windows.iter().position(|w| w == &target) {
            self.active_index = pos;
            self.active_floating = None;
        } else if self.floating_windows.iter().any(|w| w == &target) {
            self.active_floating = Some(target);
        }
        self.active_window()
    }

    /// Switch active window to previous in cycle (cycles through all tiled and floating windows)
    pub fn prev_window(&mut self) -> Option<&Window> {
        let total = self.windows.len() + self.floating_windows.len();
        if total == 0 {
            return None;
        }

        let current_active = self.active_window().cloned();
        let current_pos = current_active.as_ref().and_then(|act| {
            if let Some(pos) = self.windows.iter().position(|w| w == act) {
                Some(pos)
            } else {
                self.floating_windows.iter().position(|w| w == act).map(|p| self.windows.len() + p)
            }
        }).unwrap_or(0);

        let prev_pos = if current_pos == 0 {
            total - 1
        } else {
            current_pos - 1
        };
        let target = if prev_pos < self.windows.len() {
            self.windows[prev_pos].clone()
        } else {
            self.floating_windows[prev_pos - self.windows.len()].clone()
        };

        if let Some(pos) = self.windows.iter().position(|w| w == &target) {
            self.active_index = pos;
            self.active_floating = None;
        } else if self.floating_windows.iter().any(|w| w == &target) {
            self.active_floating = Some(target);
        }
        self.active_window()
    }

    /// Swap two specific tiled windows in the tiling order (used for drag-to-swap).
    pub fn swap_windows(&mut self, a: &Window, b: &Window) -> bool {
        if a == b {
            return false;
        }
        let pos_a = self.windows.iter().position(|w| w == a);
        let pos_b = self.windows.iter().position(|w| w == b);
        if let (Some(ia), Some(ib)) = (pos_a, pos_b) {
            self.windows.swap(ia, ib);
            if self.active_index == ia {
                self.active_index = ib;
            } else if self.active_index == ib {
                self.active_index = ia;
            }
            return true;
        }
        false
    }

    /// Swap the active window with the next window in the tiling order.
    pub fn swap_next(&mut self) {
        if self.windows.len() < 2 {
            return;
        }
        let next = (self.active_index + 1) % self.windows.len();
        self.windows.swap(self.active_index, next);
        self.active_index = next;
    }

    /// Swap the active window with the previous window in the tiling order.
    pub fn swap_prev(&mut self) {
        if self.windows.len() < 2 {
            return;
        }
        let prev = if self.active_index == 0 {
            self.windows.len() - 1
        } else {
            self.active_index - 1
        };
        self.windows.swap(self.active_index, prev);
        self.active_index = prev;
    }

    /// Promote the active window to the Master slot (index 0), or swap index 0 and 1 if already master.
    pub fn swap_master(&mut self) {
        if self.windows.len() < 2 {
            return;
        }
        if self.active_index == 0 {
            self.windows.swap(0, 1);
            self.active_index = 1;
        } else {
            self.windows.swap(0, self.active_index);
            self.active_index = 0;
        }
    }

    /// Move the currently active window to `target_workspace` (1..=9) without switching the active workspace.
    pub fn move_active_to_workspace(&mut self, target_workspace: usize) -> Option<Window> {
        let target_workspace = target_workspace.clamp(1, 9);
        if target_workspace == self.workspace {
            return None;
        }
        if !self.windows.is_empty() {
            let idx = self.active_index.min(self.windows.len() - 1);
            let win = self.windows.remove(idx);
            let wid = window_stable_id(&win);
            self.animated_rects.remove(&wid);
            if self.active_index >= self.windows.len() && !self.windows.is_empty() {
                self.active_index = self.windows.len() - 1;
            }
            self.workspace_windows
                .entry(target_workspace)
                .or_default()
                .push(win.clone());
            return Some(win);
        }
        if let Some(win) = self.floating_windows.pop() {
            self.workspace_floating
                .entry(target_workspace)
                .or_default()
                .push(win.clone());
            return Some(win);
        }
        None
    }

    /// Minimize a specific window directly to workspace 9 background storage
    pub fn minimize_window(&mut self, window: &Window) -> bool {
        let wid = window_stable_id(window);
        self.animated_rects.remove(&wid);
        if let Some(pos) = self.windows.iter().position(|w| w == window) {
            let win = self.windows.remove(pos);
            if self.active_index >= self.windows.len() && !self.windows.is_empty() {
                self.active_index = self.windows.len() - 1;
            }
            self.workspace_windows.entry(9).or_default().push(win);
            return true;
        }
        if let Some(pos) = self.floating_windows.iter().position(|w| w == window) {
            let win = self.floating_windows.remove(pos);
            if self.active_floating.as_ref() == Some(window) {
                self.active_floating = None;
            }
            self.workspace_floating.entry(9).or_default().push(win);
            return true;
        }
        false
    }

    /// Set fullscreen state for a window by stable ID.
    pub fn set_fullscreen(&mut self, window: &Window, fullscreen: bool) {
        let wid = window_stable_id(window);
        if fullscreen {
            self.fullscreen_windows.insert(wid);
        } else {
            self.fullscreen_windows.remove(&wid);
        }
    }

    /// Toggle fullscreen state for a window and return the new state.
    pub fn toggle_fullscreen(&mut self, window: &Window) -> bool {
        let wid = window_stable_id(window);
        if self.fullscreen_windows.contains(&wid) {
            self.fullscreen_windows.remove(&wid);
            false
        } else {
            self.fullscreen_windows.insert(wid);
            true
        }
    }

    /// Check whether a window is currently marked fullscreen.
    pub fn is_fullscreen(&self, window: &Window) -> bool {
        self.fullscreen_windows.contains(&window_stable_id(window))
    }

    /// Check whether a window is currently marked maximized.
    pub fn is_maximized(&self, window: &Window) -> bool {
        self.maximized_windows.contains(&window_stable_id(window))
    }

    /// Set maximized state for a window.
    pub fn set_maximized(
        &mut self,
        window: &Window,
        usable_area: Rectangle<i32, Logical>,
        maximized: bool,
    ) -> Rectangle<i32, Logical> {
        let wid = window_stable_id(window);
        let target_rect = if maximized {
            if let Some(&current_geo) = self.floating_geometries.get(&wid) {
                self.saved_unmaximized_geometries.insert(wid, current_geo);
            }
            self.maximized_windows.insert(wid);
            self.set_floating_geometry(window, usable_area);
            usable_area
        } else {
            self.maximized_windows.remove(&wid);
            let restored = self.saved_unmaximized_geometries.remove(&wid).unwrap_or_else(|| {
                Rectangle::new(
                    Point::from((usable_area.loc.x + 40, usable_area.loc.y + 40)),
                    Size::from((
                        (usable_area.size.w - 80).max(400),
                        (usable_area.size.h - 80).max(300),
                    )),
                )
            });
            self.set_floating_geometry(window, restored);
            restored
        };

        // Coalesce state + size into a single configure to avoid client flicker.
        if let Some(toplevel) = window.toplevel() {
            let uses_ssd = window_uses_ssd(window);
            let titlebar_h = if !uses_ssd { 0 } else { TITLEBAR_HEIGHT };
            let client_h = (target_rect.size.h - titlebar_h).max(100);
            toplevel.with_pending_state(|tstate| {
                use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State;
                if maximized {
                    tstate.states.set(State::Maximized);
                    tstate.states.unset(State::Fullscreen);
                } else {
                    tstate.states.unset(State::Maximized);
                }
                tstate.size = Some((target_rect.size.w.max(160), client_h).into());
            });
            toplevel.send_configure();
        } else if let Some(x11) = window.x11_surface() {
            let _ = x11.set_maximized(maximized);
            configure_window_geometry(window, target_rect);
        }

        target_rect
    }

    /// Toggle maximize state for a window and return the new state.
    pub fn toggle_maximize(
        &mut self,
        window: &Window,
        usable_area: Rectangle<i32, Logical>,
    ) -> bool {
        let wid = window_stable_id(window);
        let will_maximize = !self.maximized_windows.contains(&wid);
        self.set_maximized(window, usable_area, will_maximize);
        will_maximize
    }

    /// Return the active window if it is currently fullscreen.
    pub fn active_fullscreen_window(&self) -> Option<Window> {
        self.active_window()
            .filter(|w| self.is_fullscreen(w))
            .cloned()
    }

    /// Set or update the position/size of a floating window (and any siblings in its tab group)
    pub fn set_floating_geometry(&mut self, window: &Window, rect: Rectangle<i32, Logical>) {
        let win_id = window_stable_id(window);
        if self.floating_windows.iter().any(|w| w == window) {
            self.floating_geometries.insert(win_id, rect);
        } else {
            // If window was tiled, automatically promote it to floating when dragged
            self.remove_window(window);
            self.floating_windows.push(window.clone());
            self.floating_geometries.insert(win_id, rect);
        }
        // Keep all siblings in the same tab group synchronized to this frame geometry
        if let Some(group) = self.tab_groups.iter().find(|g| g.contains(&win_id)).cloned() {
            for sibling_id in group {
                self.floating_geometries.insert(sibling_id, rect);
            }
        }
    }

    /// Returns true if the window is currently managed in an active multi-window tiled layout.
    pub fn is_window_tiled(&self, window: &Window) -> bool {
        self.mode != LayoutMode::Floating
            && !self.is_floating(window)
            && self.windows.len() > 1
    }

    /// Merges `guest_id` into `host_id`'s tab group (creating a new group if `host_id` is not yet grouped).
    pub fn merge_windows_into_tab_group(&mut self, host_id: u64, guest_id: u64) -> bool {
        if host_id == guest_id {
            return false;
        }
        self.detach_window_from_tab_group(guest_id);
        if let Some(group) = self.tab_groups.iter_mut().find(|g| g.contains(&host_id)) {
            if !group.contains(&guest_id) {
                group.push(guest_id);
            }
        } else {
            self.tab_groups.push(vec![host_id, guest_id]);
        }
        if let Some(&host_rect) = self.floating_geometries.get(&host_id) {
            self.floating_geometries.insert(guest_id, host_rect);
        }
        true
    }

    /// Detaches `win_id` from any tab group it belongs to, removing singleton groups.
    pub fn detach_window_from_tab_group(&mut self, win_id: u64) -> bool {
        let mut detached = false;
        for group in &mut self.tab_groups {
            let before = group.len();
            group.retain(|&id| id != win_id);
            if group.len() != before {
                detached = true;
            }
        }
        self.tab_groups.retain(|g| g.len() >= 2);
        detached
    }

    /// Returns the slice of window IDs in the tab group containing `win_id`, if any.
    pub fn tab_group_for_window(&self, win_id: u64) -> Option<&[u64]> {
        self.tab_groups
            .iter()
            .find(|g| g.len() >= 2 && g.contains(&win_id))
            .map(Vec::as_slice)
    }

    /// Returns metadata `(member_id, label, is_current)` for all tabs in `win_id`'s tab group.
    pub fn window_tabs_metadata(&self, win_id: u64) -> Vec<(u64, String, bool)> {
        let Some(group) = self.tab_group_for_window(win_id) else {
            return Vec::new();
        };
        group
            .iter()
            .map(|&member_id| {
                let label = self
                    .windows
                    .iter()
                    .chain(self.floating_windows.iter())
                    .find(|w| window_stable_id(w) == member_id)
                    .map(|w| {
                        let (app, title) = window_metadata(w);
                        if !title.trim().is_empty() {
                            title.trim().to_string()
                        } else if !app.trim().is_empty() {
                            app.trim().to_string()
                        } else {
                            format!("Tab {}", member_id % 100)
                        }
                    })
                    .unwrap_or_else(|| format!("Tab {}", member_id % 100));
                (member_id, label, member_id == win_id)
            })
            .collect()
    }

    pub fn floating_geometry(&self, window: &Window) -> Option<Rectangle<i32, Logical>> {
        self.floating_geometries.get(&window_stable_id(window)).copied()
    }

    /// Return slice of floating windows on the active workspace.
    pub fn floating_windows(&self) -> &[Window] {
        &self.floating_windows
    }

    /// Remove an X11 surface from tiled/floating lists across all workspaces.
    pub fn remove_x11_surface(&mut self, wid: u64) -> bool {
        let mut removed = false;
        let before_tiled = self.windows.len();
        self.windows.retain(|w| window_stable_id(w) != wid);
        if self.windows.len() != before_tiled {
            removed = true;
            if self.active_index >= self.windows.len() && !self.windows.is_empty() {
                self.active_index = self.windows.len() - 1;
            }
        }
        let before_float = self.floating_windows.len();
        self.floating_windows.retain(|w| window_stable_id(w) != wid);
        if self.floating_windows.len() != before_float {
            removed = true;
        }
        for list in self.workspace_windows.values_mut() {
            let b = list.len();
            list.retain(|w| window_stable_id(w) != wid);
            if list.len() != b {
                removed = true;
            }
        }
        for list in self.workspace_floating.values_mut() {
            let b = list.len();
            list.retain(|w| window_stable_id(w) != wid);
            if list.len() != b {
                removed = true;
            }
        }
        self.animated_rects.remove(&wid);
        self.animated_alphas.remove(&wid);
        self.target_alphas.remove(&wid);
        self.floating_geometries.remove(&wid);
        removed
    }

    /// Focuses a window by its stable ID, switching workspaces if needed.
    /// Returns the matched `Window` if found and raises it to top.
    pub fn focus_window_by_id(&mut self, target_id: u64) -> Option<(usize, Window)> {
        if let Some(pos) = self
            .windows
            .iter()
            .position(|w| window_stable_id(w) == target_id)
        {
            self.active_index = pos;
            self.active_floating = None;
            return Some((self.workspace, self.windows[pos].clone()));
        }
        if let Some(pos) = self
            .floating_windows
            .iter()
            .position(|w| window_stable_id(w) == target_id)
        {
            let w = self.floating_windows.remove(pos);
            self.floating_windows.push(w.clone());
            self.active_floating = Some(w.clone());
            return Some((self.workspace, w));
        }
        let target_ws = self
            .workspace_windows
            .iter()
            .find_map(|(&ws, list)| {
                list.iter()
                    .any(|w| window_stable_id(w) == target_id)
                    .then_some(ws)
            })
            .or_else(|| {
                self.workspace_floating.iter().find_map(|(&ws, list)| {
                    list.iter()
                        .any(|w| window_stable_id(w) == target_id)
                        .then_some(ws)
                })
            })?;

        if target_ws == 9 {
            // Window was minimized to workspace 9: restore it to current workspace
            let current_ws = self.workspace;
            if let Some(list) = self.workspace_windows.get_mut(&9) {
                if let Some(pos) = list.iter().position(|w| window_stable_id(w) == target_id) {
                    let w = list.remove(pos);
                    self.add_window(w.clone());
                    return Some((current_ws, w));
                }
            }
            if let Some(list) = self.workspace_floating.get_mut(&9) {
                if let Some(pos) = list.iter().position(|w| window_stable_id(w) == target_id) {
                    let w = list.remove(pos);
                    self.add_floating_window(w.clone());
                    return Some((current_ws, w));
                }
            }
        }

        self.switch_workspace(target_ws);
        if let Some(pos) = self
            .windows
            .iter()
            .position(|w| window_stable_id(w) == target_id)
        {
            self.active_index = pos;
            self.active_floating = None;
            return Some((self.workspace, self.windows[pos].clone()));
        }
        if let Some(pos) = self
            .floating_windows
            .iter()
            .position(|w| window_stable_id(w) == target_id)
        {
            let w = self.floating_windows.remove(pos);
            self.floating_windows.push(w.clone());
            self.active_floating = Some(w.clone());
            return Some((self.workspace, w));
        }
        None
    }

    /// Returns sorted list of workspaces (1..=9) that currently contain at least one window.
    pub fn occupied_workspaces(&self) -> Vec<u32> {
        let mut occupied = Vec::new();
        if !self.windows.is_empty() || !self.floating_windows.is_empty() {
            occupied.push(self.workspace as u32);
        }
        for (&ws, list) in &self.workspace_windows {
            if !list.is_empty() && !occupied.contains(&(ws as u32)) {
                occupied.push(ws as u32);
            }
        }
        for (&ws, list) in &self.workspace_floating {
            if !list.is_empty() && !occupied.contains(&(ws as u32)) {
                occupied.push(ws as u32);
            }
        }
        occupied.sort_unstable();
        occupied
    }

    /// Collects metadata snapshots of all managed windows across all workspaces.
    pub fn window_snapshots(&self) -> Vec<WindowSnapshot> {
        let mut snapshots = Vec::new();
        let active_win = self.active_window();

        for window in &self.windows {
            let (app_id, title) = window_metadata(window);
            snapshots.push(WindowSnapshot {
                id: window_stable_id(window),
                app_id,
                title,
                workspace: self.workspace,
                is_active: active_win == Some(window),
                is_floating: false,
            });
        }
        for window in &self.floating_windows {
            let (app_id, title) = window_metadata(window);
            snapshots.push(WindowSnapshot {
                id: window_stable_id(window),
                app_id,
                title,
                workspace: self.workspace,
                is_active: active_win == Some(window),
                is_floating: true,
            });
        }
        for (&ws, list) in &self.workspace_windows {
            if ws == self.workspace {
                continue;
            }
            for window in list {
                let (app_id, title) = window_metadata(window);
                snapshots.push(WindowSnapshot {
                    id: window_stable_id(window),
                    app_id,
                    title,
                    workspace: ws,
                    is_active: false,
                    is_floating: false,
                });
            }
        }
        for (&ws, list) in &self.workspace_floating {
            if ws == self.workspace {
                continue;
            }
            for window in list {
                let (app_id, title) = window_metadata(window);
                snapshots.push(WindowSnapshot {
                    id: window_stable_id(window),
                    app_id,
                    title,
                    workspace: ws,
                    is_active: false,
                    is_floating: true,
                });
            }
        }
        snapshots
    }

    /// Sets a persistent target opacity for a window (from window rules or direct user configuration).
    pub fn set_window_target_alpha(&mut self, window: &Window, alpha: f32) {
        let wid = window_stable_id(window);
        self.target_alphas.insert(wid, alpha.clamp(0.1, 1.0));
    }

    /// Returns the target opacity configured for a window (defaults to 1.0).
    pub fn window_target_alpha(&self, window: &Window) -> f32 {
        let wid = window_stable_id(window);
        self.target_alphas.get(&wid).copied().unwrap_or(1.0)
    }

    /// Returns the current animated opacity (`0.15..=1.0`) for a window during spawn/fade transition.
    pub fn window_alpha(&self, window: &Window) -> f32 {
        let win_id = window_stable_id(window);
        self.animated_alphas.get(&win_id).copied().unwrap_or(1.0)
    }

    /// Returns true if any window is still mid-transition toward its target geometry or opacity.
    #[allow(dead_code)]
    pub fn is_animating(&self, usable_area: Rectangle<i32, Logical>) -> bool {
        if self.workspace_slide_offset.abs() > 0.75 {
            return true;
        }
        let targets = self.arrange(usable_area);
        for (window, target_rect) in targets {
            let win_id = window_stable_id(&window);
            let Some(current) = self.animated_rects.get(&win_id) else {
                return true;
            };
            if (current.loc.x - target_rect.loc.x as f32).abs() > 0.75
                || (current.loc.y - target_rect.loc.y as f32).abs() > 0.75
                || (current.size.w - target_rect.size.w as f32).abs() > 0.75
                || (current.size.h - target_rect.size.h as f32).abs() > 0.75
            {
                return true;
            }
            let target_alpha = self.window_target_alpha(&window);
            let alpha = self.animated_alphas.get(&win_id).copied().unwrap_or(1.0);
            if (target_alpha - alpha).abs() > 0.015 {
                return true;
            }
        }
        for window in &self.floating_windows {
            let win_id = window_stable_id(window);
            let target_alpha = self.window_target_alpha(window);
            let alpha = self.animated_alphas.get(&win_id).copied().unwrap_or(1.0);
            if (target_alpha - alpha).abs() > 0.015 {
                return true;
            }
        }
        false
    }

    /// Adjust master ratio by delta (clamped between 0.2 and 0.8)
    pub fn adjust_master_ratio(&mut self, delta: f32) {
        self.master_ratio = (self.master_ratio + delta).clamp(0.2, 0.8);
    }

    /// Returns current master ratio
    pub fn master_ratio(&self) -> f32 {
        self.master_ratio
    }

    /// Sets the master ratio explicitly (clamped between 0.2 and 0.8)
    pub fn set_master_ratio(&mut self, ratio: f32) {
        self.master_ratio = ratio.clamp(0.2, 0.8);
    }

    /// Calculates the horizontal split separator x-coordinate in MasterStack mode
    pub fn split_x(&self, usable_area: Rectangle<i32, Logical>) -> Option<i32> {
        if self.windows.len() <= 1 || self.mode != LayoutMode::MasterStack {
            return None;
        }
        let total_w = usable_area.size.w;
        let g = self.gap;
        let master_w = (((total_w - g) as f32) * self.master_ratio).round() as i32;
        Some(usable_area.loc.x + master_w + g / 2)
    }

    /// Computes the layout geometries for all managed windows within the usable area.
    ///
    /// Supports three layout modes:
    /// - `MasterStack`: Master on left (`master_ratio`), vertical stack on right
    /// - `Dwindle`: Fibonacci/Spiral recursive binary split alternating horizontal and vertical
    /// - `Monocle`: Active window fills usable area (or all windows stacked full-size with active on top)
    pub fn arrange(
        &self,
        usable_area: Rectangle<i32, Logical>,
    ) -> Vec<(Window, Rectangle<i32, Logical>)> {
        let count = self.windows.len();
        if count == 0 {
            return Vec::new();
        }

        let mut results = Vec::with_capacity(count);
        let g = self.gap;

        // Effective inner usable area with outer margin
        let total_x = usable_area.loc.x + g;
        let total_y = usable_area.loc.y + g;
        let total_w = (usable_area.size.w - 2 * g).max(100);
        let total_h = (usable_area.size.h - 2 * g).max(100);

        if count == 1 {
            let win = &self.windows[0];
            let win_id = window_stable_id(win);
            let rect = match self.mode() {
                LayoutMode::Floating => {
                    if let Some(&saved) = self.floating_geometries.get(&win_id) {
                        saved
                    } else {
                        let win_geo = win.geometry();
                        let titlebar_h = window_titlebar_height(win, false);
                        let (w, h) = if win_geo.size.w > 0 && win_geo.size.h > 0 {
                            (win_geo.size.w, win_geo.size.h + titlebar_h)
                        } else {
                            (960.min(total_w - 40), 640.min(total_h - 40))
                        };
                        Rectangle::new(
                            Point::from((total_x + 36, total_y + 36)),
                            Size::from((w, h)),
                        )
                    }
                }
                LayoutMode::Monocle => Rectangle::new(
                    Point::from((usable_area.loc.x, usable_area.loc.y)),
                    Size::from((usable_area.size.w, usable_area.size.h)),
                ),
                LayoutMode::Scrollable => {
                    let col_w = ((total_w as f32) * self.scroll_column_fraction.clamp(0.25, 1.0))
                        .round()
                        .max(320.0) as i32;
                    let center_x = total_x + (total_w - col_w).max(0) / 2;
                    Rectangle::new(Point::from((center_x, total_y)), Size::from((col_w, total_h)))
                }
                LayoutMode::MasterStack | LayoutMode::Dwindle | LayoutMode::Bsp => Rectangle::new(
                    Point::from((total_x, total_y)),
                    Size::from((total_w, total_h)),
                ),
            };
            results.push((win.clone(), rect));
            return results;
        }

        match self.mode() {
            LayoutMode::Scrollable => {
                // Niri-style infinite horizontal strip of columns
                let col_w = ((total_w as f32) * self.scroll_column_fraction.clamp(0.25, 1.0))
                    .round()
                    .max(320.0) as i32;
                let step = col_w + self.gap;
                let max_scroll = (count.saturating_sub(1) as f64 * step as f64).max(0.0);
                let scroll_x = self.scroll_offset_x.clamp(0.0, max_scroll).round() as i32;

                for (idx, window) in self.windows.iter().enumerate() {
                    let x = total_x + (idx as i32 * step) - scroll_x;
                    let rect = Rectangle::new(
                        Point::from((x, total_y)),
                        Size::from((col_w, total_h)),
                    );
                    results.push((window.clone(), rect));
                }
                results
            }
            LayoutMode::Floating => {
                for (i, window) in self.windows.iter().enumerate() {
                    let win_id = window_stable_id(window);
                    let rect = if let Some(&saved) = self.floating_geometries.get(&win_id) {
                        saved
                    } else {
                        let win_geo = window.geometry();
                        let (w, h) = if win_geo.size.w > 0 && win_geo.size.h > 0 {
                            (win_geo.size.w, win_geo.size.h + 36)
                        } else {
                            (960.min(total_w - 40), 640.min(total_h - 40))
                        };
                        let max_cascade_x = (total_w.saturating_sub(w) - 64).max(0);
                        let max_cascade_y = (total_h.saturating_sub(h) - 64).max(0);
                        let cascade_x = if max_cascade_x > 0 { (i as i32 * 52) % max_cascade_x } else { 0 };
                        let cascade_y = if max_cascade_y > 0 { (i as i32 * 44) % max_cascade_y } else { 0 };
                        Rectangle::new(
                            Point::from((total_x + 36 + cascade_x, total_y + 36 + cascade_y)),
                            Size::from((w, h)),
                        )
                    };
                    results.push((window.clone(), rect));
                }
                results
            }
            LayoutMode::Monocle => {
                let full_rect = Rectangle::new(
                    Point::from((total_x, total_y)),
                    Size::from((total_w, total_h)),
                );
                let active_idx = self.active_index.min(count - 1);
                for (idx, window) in self.windows.iter().enumerate() {
                    if idx != active_idx {
                        results.push((window.clone(), full_rect));
                    }
                }
                results.push((self.windows[active_idx].clone(), full_rect));
                results
            }
            LayoutMode::Dwindle => {
                let mut rem_x = total_x;
                let mut rem_y = total_y;
                let mut rem_w = total_w;
                let mut rem_h = total_h;

                for (idx, window) in self.windows.iter().enumerate() {
                    if idx == count - 1 {
                        let rect = Rectangle::new(
                            Point::from((rem_x, rem_y)),
                            Size::from((rem_w.max(50), rem_h.max(50))),
                        );
                        results.push((window.clone(), rect));
                    } else if idx % 2 == 0 {
                        // Split horizontally (left / right)
                        let ratio = if idx == 0 { self.master_ratio } else { 0.5 };
                        let cur_w = (((rem_w - g).max(100) as f32) * ratio).round() as i32;
                        let rect = Rectangle::new(
                            Point::from((rem_x, rem_y)),
                            Size::from((cur_w.max(50), rem_h.max(50))),
                        );
                        results.push((window.clone(), rect));
                        rem_x += cur_w + g;
                        rem_w = (rem_w - cur_w - g).max(50);
                    } else {
                        // Split vertically (top / bottom)
                        let cur_h = ((rem_h - g).max(100)) / 2;
                        let rect = Rectangle::new(
                            Point::from((rem_x, rem_y)),
                            Size::from((rem_w.max(50), cur_h.max(50))),
                        );
                        results.push((window.clone(), rect));
                        rem_y += cur_h + g;
                        rem_h = (rem_h - cur_h - g).max(50);
                    }
                }
                results
            }
            LayoutMode::MasterStack => {
                // Master window width
                let master_w = (((total_w - g) as f32) * self.master_ratio).round() as i32;
                let stack_w = total_w - g - master_w;

                // 1. Master window on the left
                let master_rect = Rectangle::new(
                    Point::from((total_x, total_y)),
                    Size::from((master_w, total_h)),
                );
                results.push((self.windows[0].clone(), master_rect));

                // 2. Stack windows on the right
                let stack_count = (count - 1) as i32;
                let stack_total_gaps = (stack_count - 1) * g;
                let stack_slot_h = (total_h - stack_total_gaps) / stack_count;
                let stack_x = total_x + master_w + g;

                for (i, window) in self.windows.iter().skip(1).enumerate() {
                    let idx = i as i32;
                    let stack_y = total_y + idx * (stack_slot_h + g);
                    // Give remaining height to last window to avoid rounding gap
                    let h = if idx == stack_count - 1 {
                        total_y + total_h - stack_y
                    } else {
                        stack_slot_h
                    };

                    let rect = Rectangle::new(
                        Point::from((stack_x, stack_y)),
                        Size::from((stack_w, h.max(50))),
                    );
                    results.push((window.clone(), rect));
                }

                results
            }
            LayoutMode::Bsp => {
                let mut bsp = self.bsp_tree.clone();
                let active_ids: Vec<u64> = self.windows.iter().map(window_stable_id).collect();
                if bsp.is_none() && !active_ids.is_empty() {
                    bsp = Some(BspNode::Leaf(active_ids[0]));
                }
                if let Some(ref mut tree) = bsp {
                    for &id in &active_ids {
                        if !tree.contains(id) {
                            tree.insert_default(id, self.split_direction);
                        }
                    }
                    let full_rect = Rectangle::new(
                        Point::from((total_x, total_y)),
                        Size::from((total_w, total_h)),
                    );
                    let mut rects = HashMap::new();
                    tree.compute_rects(full_rect, g, &mut rects);
                    for window in &self.windows {
                        let id = window_stable_id(window);
                        if let Some(&rect) = rects.get(&id) {
                            results.push((window.clone(), rect));
                        } else {
                            results.push((window.clone(), full_rect));
                        }
                    }
                } else {
                    for window in &self.windows {
                        results.push((
                            window.clone(),
                            Rectangle::new(Point::from((total_x, total_y)), Size::from((total_w, total_h))),
                        ));
                    }
                }
                results
            }
        }
    }

    /// Overview / Exposé layout mode:
    /// Arranges all windows in an optimal balanced grid with generous margins
    /// so that every window is completely visible at a glance.
    pub fn arrange_overview(
        &self,
        usable_area: Rectangle<i32, Logical>,
    ) -> Vec<(Window, Rectangle<i32, Logical>)> {
        let count = self.windows.len();
        if count == 0 {
            return Vec::new();
        }

        let margin = 48;
        let gap = 24;

        let total_x = usable_area.loc.x + margin;
        let total_y = usable_area.loc.y + margin;
        let total_w = (usable_area.size.w - 2 * margin).max(200);
        let total_h = (usable_area.size.h - 2 * margin).max(200);

        let cols = match count {
            1 => 1,
            2..=4 => 2,
            5..=9 => 3,
            _ => 4,
        };
        let rows = count.div_ceil(cols);

        let cell_w = (total_w - (cols as i32 - 1) * gap) / cols as i32;
        let cell_h = (total_h - (rows as i32 - 1) * gap) / rows as i32;

        let mut results = Vec::with_capacity(count);
        for (i, window) in self.windows.iter().enumerate() {
            let col = (i % cols) as i32;
            let row = (i / cols) as i32;

            let x = total_x + col * (cell_w + gap);
            let y = total_y + row * (cell_h + gap);

            let rect = Rectangle::new(
                Point::from((x, y)),
                Size::from((cell_w.max(100), cell_h.max(100))),
            );
            results.push((window.clone(), rect));
        }

        results
    }

    /// Computes animated geometries with critically-damped spring physics interpolation (stiffness ~ 0.32)
    pub fn arrange_animated(
        &mut self,
        usable_area: Rectangle<i32, Logical>,
    ) -> Vec<(Window, Rectangle<i32, Logical>)> {
        let targets = self.arrange(usable_area);
        let mut animated_results = Vec::with_capacity(targets.len());

        let spring_stiffness = 0.28; // Silky critically-damped 60-120fps spring interpolation
        let alpha_stiffness = 0.26_f32;
        self.workspace_slide_offset *= 0.68;
        if self.workspace_slide_offset.abs() < 0.75 {
            self.workspace_slide_offset = 0.0;
        }
        let slide_x = self.workspace_slide_offset.round() as i32;

        for (window, target_rect) in targets {
            let win_id = window_stable_id(&window);
            let target_alpha = self.window_target_alpha(&window);
            let alpha_entry = self.animated_alphas.entry(win_id).or_insert(0.15);
            *alpha_entry += (target_alpha - *alpha_entry) * alpha_stiffness;
            if (target_alpha - *alpha_entry).abs() < 0.015 {
                *alpha_entry = target_alpha;
            }
            let is_fs = self.is_fullscreen(&window);
            let target_fx = target_rect.loc.x as f32;
            let target_fy = target_rect.loc.y as f32;
            let target_fw = target_rect.size.w as f32;
            let target_fh = target_rect.size.h as f32;

            let current_entry = self.animated_rects.entry(win_id).or_insert_with(|| {
                // Smooth slide-in + scale-up spawn animation (90% scale, slight slide offset)
                let spawn_inset_x = target_fw * 0.05 + 28.0;
                let spawn_inset_y = target_fh * 0.05 + 14.0;
                Rectangle::new(
                    Point::from((target_fx + spawn_inset_x, target_fy + spawn_inset_y)),
                    Size::from((target_fw * 0.90, target_fh * 0.90)),
                )
            });

            // Exponential decay / spring step
            current_entry.loc.x += (target_fx - current_entry.loc.x) * spring_stiffness;
            current_entry.loc.y += (target_fy - current_entry.loc.y) * spring_stiffness;
            current_entry.size.w += (target_fw - current_entry.size.w) * spring_stiffness;
            current_entry.size.h += (target_fh - current_entry.size.h) * spring_stiffness;

            let final_rect = if is_fs {
                Rectangle::new(
                    Point::from((0, 0)),
                    Size::from((
                        (usable_area.loc.x + usable_area.size.w).max(100),
                        (usable_area.loc.y + usable_area.size.h).max(100),
                    )),
                )
            } else {
                Rectangle::new(
                    Point::from((
                        (current_entry.loc.x.round() as i32) + slide_x,
                        current_entry.loc.y.round() as i32,
                    )),
                    Size::from((
                        current_entry.size.w.round() as i32,
                        current_entry.size.h.round() as i32,
                    )),
                )
            };

            animated_results.push((window, final_rect));
        }

        // Include floating windows centered or at their saved geometries with spring spawn/maximize physics
        for (i, window) in self.floating_windows.iter().enumerate() {
            let win_id = window_stable_id(window);
            let target_alpha = self.window_target_alpha(window);
            let alpha_entry = self.animated_alphas.entry(win_id).or_insert(0.15);
            *alpha_entry += (target_alpha - *alpha_entry) * alpha_stiffness;
            if (target_alpha - *alpha_entry).abs() < 0.015 {
                *alpha_entry = target_alpha;
            }
            let is_fs = self.is_fullscreen(window);
            let float_rect = if is_fs {
                Rectangle::new(
                    Point::from((0, 0)),
                    Size::from((
                        (usable_area.loc.x + usable_area.size.w).max(100),
                        (usable_area.loc.y + usable_area.size.h).max(100),
                    )),
                )
            } else if self.is_maximized(window) {
                usable_area
            } else if let Some(&saved) = self.floating_geometries.get(&win_id) {
                saved
            } else {
                let win_geo = window.geometry();
                let (mut w, mut h) = if win_geo.size.w > 0 && win_geo.size.h > 0 {
                    (win_geo.size.w, win_geo.size.h + 36)
                } else {
                    (960.min(usable_area.size.w - 48), 640.min(usable_area.size.h - 48))
                };
                w = w.min(usable_area.size.w - 48).max(200);
                h = h.min(usable_area.size.h - 48).max(200);

                // Smart placement: Cascade with distinct 52px horizontal & 44px vertical steps
                // Prevents any overlap of titlebars, controls or window content
                let max_cascade_x = (usable_area.size.w.saturating_sub(w) - 64).max(0);
                let max_cascade_y = (usable_area.size.h.saturating_sub(h) - 64).max(0);
                let step_x = 52;
                let step_y = 44;
                let cascade_x = if max_cascade_x > 0 { (i as i32 * step_x) % max_cascade_x } else { 0 };
                let cascade_y = if max_cascade_y > 0 { (i as i32 * step_y) % max_cascade_y } else { 0 };
                let x = usable_area.loc.x + 36 + cascade_x;
                let y = usable_area.loc.y + 36 + cascade_y;
                let initial_rect = Rectangle::new(Point::from((x, y)), Size::from((w, h)));
                self.floating_geometries.insert(win_id, initial_rect);
                initial_rect
            };

            let target_fx = float_rect.loc.x as f32;
            let target_fy = float_rect.loc.y as f32;
            let target_fw = float_rect.size.w as f32;
            let target_fh = float_rect.size.h as f32;

            let current_entry = self.animated_rects.entry(win_id).or_insert_with(|| {
                let spawn_inset_x = target_fw * 0.03;
                let spawn_inset_y = target_fh * 0.03;
                Rectangle::new(
                    Point::from((target_fx + spawn_inset_x, target_fy + spawn_inset_y)),
                    Size::from((target_fw * 0.94, target_fh * 0.94)),
                )
            });

            current_entry.loc.x += (target_fx - current_entry.loc.x) * 0.38;
            current_entry.loc.y += (target_fy - current_entry.loc.y) * 0.38;
            current_entry.size.w += (target_fw - current_entry.size.w) * 0.38;
            current_entry.size.h += (target_fh - current_entry.size.h) * 0.38;

            let animated_float_rect = if is_fs {
                float_rect
            } else {
                Rectangle::new(
                    Point::from((
                        (current_entry.loc.x.round() as i32) + slide_x,
                        current_entry.loc.y.round() as i32,
                    )),
                    Size::from((
                        current_entry.size.w.round() as i32,
                        current_entry.size.h.round() as i32,
                    )),
                )
            };

            animated_results.push((window.clone(), animated_float_rect));
        }

        // Raise the active window to the very top of the Z-order:
        // By placing it at the end of animated_results, the compositor's `.rev()` front-to-back
        // iteration processes the active window first, ensuring it is drawn on top of all other windows!
        if let Some(active) = self.active_window().cloned() {
            if let Some(pos) = animated_results.iter().position(|(w, _)| w == &active) {
                let active_item = animated_results.remove(pos);
                animated_results.push(active_item);
            }
        }

        animated_results
    }
}

/// Detects if cursor is hovered within the split drop zones (25% inner edge) of any target window.
/// Returns (target_window, split_direction, new_on_left_or_top, preview_rectangle).
pub fn detect_drag_split_target(
    cursor: Point<f64, Logical>,
    active_layouts: &[(Window, Rectangle<i32, Logical>)],
    dragged: &Window,
) -> Option<(Window, SplitDirection, bool, Rectangle<i32, Logical>)> {
    let dragged_id = window_stable_id(dragged);
    for (win, rect) in active_layouts {
        if window_stable_id(win) == dragged_id {
            continue;
        }
        let rx = rect.loc.x as f64;
        let ry = rect.loc.y as f64;
        let rw = rect.size.w as f64;
        let rh = rect.size.h as f64;

        if cursor.x >= rx && cursor.x <= rx + rw && cursor.y >= ry && cursor.y <= ry + rh {
            let edge_w = (rw * 0.25).clamp(24.0, 160.0);
            let edge_h = (rh * 0.25).clamp(24.0, 160.0);

            let d_left = cursor.x - rx;
            let d_right = (rx + rw) - cursor.x;
            let d_top = cursor.y - ry;
            let d_bottom = (ry + rh) - cursor.y;

            let min_d = d_left.min(d_right).min(d_top).min(d_bottom);

            if min_d == d_left && d_left <= edge_w {
                let half_w = (rect.size.w / 2).max(50);
                let preview = Rectangle::new(rect.loc, Size::from((half_w, rect.size.h)));
                return Some((win.clone(), SplitDirection::Horizontal, true, preview));
            } else if min_d == d_right && d_right <= edge_w {
                let half_w = (rect.size.w / 2).max(50);
                let preview = Rectangle::new(
                    Point::from((rect.loc.x + rect.size.w - half_w, rect.loc.y)),
                    Size::from((half_w, rect.size.h)),
                );
                return Some((win.clone(), SplitDirection::Horizontal, false, preview));
            } else if min_d == d_top && d_top <= edge_h {
                let half_h = (rect.size.h / 2).max(50);
                let preview = Rectangle::new(rect.loc, Size::from((rect.size.w, half_h)));
                return Some((win.clone(), SplitDirection::Vertical, true, preview));
            } else if min_d == d_bottom && d_bottom <= edge_h {
                let half_h = (rect.size.h / 2).max(50);
                let preview = Rectangle::new(
                    Point::from((rect.loc.x, rect.loc.y + rect.size.h - half_h)),
                    Size::from((rect.size.w, half_h)),
                );
                return Some((win.clone(), SplitDirection::Vertical, false, preview));
            }
        }
    }
    None
}

/// Magnetically snaps a moving window geometry to screen edges, usable area (dock/panel),
/// and neighboring windows within `snap_distance` (default 14px).
pub fn snap_window_to_edges(
    initial_candidate: Rectangle<i32, Logical>,
    usable_area: Rectangle<i32, Logical>,
    screen_bounds: Rectangle<i32, Logical>,
    other_windows: &[Rectangle<i32, Logical>],
    snap_distance: i32,
) -> Rectangle<i32, Logical> {
    let mut snapped = initial_candidate;

    // 1. Horizontal snapping targets (screen edges, dock, and all windows)
    let mut x_targets = vec![
        usable_area.loc.x,
        usable_area.loc.x + usable_area.size.w,
        screen_bounds.loc.x,
        screen_bounds.loc.x + screen_bounds.size.w,
    ];
    for other in other_windows {
        x_targets.push(other.loc.x);
        x_targets.push(other.loc.x + other.size.w);
    }

    let cand_left = snapped.loc.x;
    let cand_right = snapped.loc.x + snapped.size.w;
    let mut best_dx: Option<i32> = None;

    for &target in &x_targets {
        let delta_left = target - cand_left;
        if delta_left.abs() <= snap_distance
            && best_dx.is_none_or(|b| delta_left.abs() < b.abs())
        {
            best_dx = Some(delta_left);
        }
        let delta_right = target - cand_right;
        if delta_right.abs() <= snap_distance
            && best_dx.is_none_or(|b| delta_right.abs() < b.abs())
        {
            best_dx = Some(delta_right);
        }
    }
    if let Some(dx) = best_dx {
        snapped.loc.x += dx;
    }

    // 2. Vertical snapping targets (screen edges, dock, and all windows)
    let mut y_targets = vec![
        usable_area.loc.y,
        usable_area.loc.y + usable_area.size.h,
        screen_bounds.loc.y,
        screen_bounds.loc.y + screen_bounds.size.h,
    ];
    for other in other_windows {
        y_targets.push(other.loc.y);
        y_targets.push(other.loc.y + other.size.h);
    }

    let cand_top = snapped.loc.y;
    let cand_bottom = snapped.loc.y + snapped.size.h;
    let mut best_dy: Option<i32> = None;

    for &target in &y_targets {
        let delta_top = target - cand_top;
        if delta_top.abs() <= snap_distance
            && best_dy.is_none_or(|b| delta_top.abs() < b.abs())
        {
            best_dy = Some(delta_top);
        }
        let delta_bottom = target - cand_bottom;
        if delta_bottom.abs() <= snap_distance
            && best_dy.is_none_or(|b| delta_bottom.abs() < b.abs())
        {
            best_dy = Some(delta_bottom);
        }
    }
    if let Some(dy) = best_dy {
        snapped.loc.y += dy;
    }

    snapped
}

/// Magnetically snaps window size when resizing right and bottom borders.
pub fn snap_window_resize(
    initial_candidate: Rectangle<i32, Logical>,
    usable_area: Rectangle<i32, Logical>,
    screen_bounds: Rectangle<i32, Logical>,
    other_windows: &[Rectangle<i32, Logical>],
    snap_distance: i32,
) -> Rectangle<i32, Logical> {
    let mut snapped = initial_candidate;

    // 1. Snap right edge
    let mut x_targets = vec![
        usable_area.loc.x + usable_area.size.w,
        screen_bounds.loc.x + screen_bounds.size.w,
    ];
    for other in other_windows {
        x_targets.push(other.loc.x);
        x_targets.push(other.loc.x + other.size.w);
    }

    let cand_right = snapped.loc.x + snapped.size.w;
    let mut best_dw: Option<i32> = None;
    for &target in &x_targets {
        let delta = target - cand_right;
        if delta.abs() <= snap_distance
            && best_dw.is_none_or(|b| delta.abs() < b.abs())
        {
            best_dw = Some(delta);
        }
    }
    if let Some(dw) = best_dw {
        snapped.size.w = (snapped.size.w + dw).max(160);
    }

    // 2. Snap bottom edge
    let mut y_targets = vec![
        usable_area.loc.y + usable_area.size.h,
        screen_bounds.loc.y + screen_bounds.size.h,
    ];
    for other in other_windows {
        y_targets.push(other.loc.y);
        y_targets.push(other.loc.y + other.size.h);
    }

    let cand_bottom = snapped.loc.y + snapped.size.h;
    let mut best_dh: Option<i32> = None;
    for &target in &y_targets {
        let delta = target - cand_bottom;
        if delta.abs() <= snap_distance
            && best_dh.is_none_or(|b| delta.abs() < b.abs())
        {
            best_dh = Some(delta);
        }
    }
    if let Some(dh) = best_dh {
        snapped.size.h = (snapped.size.h + dh).max(120);
    }

    snapped
}

/// Semantic context badge displayed in the window titlebar chrome.
#[derive(Debug, Clone, PartialEq)]
pub struct TitlebarBadge {
    pub label: &'static str,
    pub is_dirty: bool,
    pub color_rgb: [f32; 3],
}

/// Extracts a dynamic context badge and unsaved/dirty indicator from a window's `(app_id, title)`.
pub fn extract_titlebar_badge(app_id: &str, title: &str) -> Option<TitlebarBadge> {
    let hay = format!("{} {}", app_id, title).to_ascii_lowercase();
    let is_dirty = title.contains('•')
        || title.contains("[+]")
        || title.contains("(modified)")
        || title.ends_with(" *")
        || title.starts_with("* ");

    if hay.contains("terminal")
        || hay.contains("alacritty")
        || hay.contains("foot")
        || hay.contains("kitty")
        || hay.contains("wezterm")
        || hay.contains("konsole")
        || hay.contains("zsh")
        || hay.contains("bash")
    {
        Some(TitlebarBadge {
            label: "SHELL",
            is_dirty,
            color_rgb: [0.28, 0.92, 0.58],
        })
    } else if hay.contains("code")
        || hay.contains("zed")
        || hay.contains("nvim")
        || hay.contains("neovide")
        || hay.contains("cursor")
        || hay.contains(".rs")
    {
        Some(TitlebarBadge {
            label: "DEV",
            is_dirty,
            color_rgb: [0.95, 0.58, 0.22],
        })
    } else if hay.contains("spotify")
        || hay.contains("vlc")
        || hay.contains("mpv")
        || hay.contains("amberol")
        || hay.contains("rhythmbox")
    {
        Some(TitlebarBadge {
            label: "AUDIO",
            is_dirty: false,
            color_rgb: [0.98, 0.72, 0.25],
        })
    } else if hay.contains("file")
        || hay.contains("dosya")
        || hay.contains("nautilus")
        || hay.contains("dolphin")
    {
        Some(TitlebarBadge {
            label: "FILES",
            is_dirty,
            color_rgb: [0.42, 0.78, 0.95],
        })
    } else if hay.contains("setting") || hay.contains("ayar") || hay.contains("config") {
        Some(TitlebarBadge {
            label: "SYS",
            is_dirty,
            color_rgb: [0.90, 0.48, 0.24],
        })
    } else if is_dirty {
        Some(TitlebarBadge {
            label: "EDIT",
            is_dirty: true,
            color_rgb: [0.95, 0.58, 0.22],
        })
    } else {
        None
    }
}

/// Interactive window snap layout presets exposed in the Maximize button Snap Flyout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapZone {
    LeftHalf,
    RightHalf,
    LeftTwoThirds,
    RightOneThird,
    TopLeftQuarter,
    BottomRightQuarter,
}

/// Computes the exact target logical rectangle for a `SnapZone` inside `usable_area`.
pub fn compute_snap_zone_rect(
    usable_area: Rectangle<i32, Logical>,
    zone: SnapZone,
    gap: i32,
) -> Rectangle<i32, Logical> {
    let g = gap.max(0);
    let x = usable_area.loc.x + g;
    let y = usable_area.loc.y + g;
    let w = (usable_area.size.w - 2 * g).max(200);
    let h = (usable_area.size.h - 2 * g).max(160);
    let half_w = (w - g) / 2;
    let half_h = (h - g) / 2;
    let two_thirds_w = ((w - g) * 2) / 3;
    let one_third_w = (w - g) - two_thirds_w;

    match zone {
        SnapZone::LeftHalf => Rectangle::new(Point::from((x, y)), Size::from((half_w, h))),
        SnapZone::RightHalf => Rectangle::new(
            Point::from((x + half_w + g, y)),
            Size::from((w - half_w - g, h)),
        ),
        SnapZone::LeftTwoThirds => {
            Rectangle::new(Point::from((x, y)), Size::from((two_thirds_w, h)))
        }
        SnapZone::RightOneThird => Rectangle::new(
            Point::from((x + two_thirds_w + g, y)),
            Size::from((one_third_w, h)),
        ),
        SnapZone::TopLeftQuarter => {
            Rectangle::new(Point::from((x, y)), Size::from((half_w, half_h)))
        }
        SnapZone::BottomRightQuarter => Rectangle::new(
            Point::from((x + half_w + g, y + half_h + g)),
            Size::from((w - half_w - g, h - half_h - g)),
        ),
    }
}

/// Returns the logical bounds of the Snap Layouts Flyout anchored below the window's Maximize button.
pub fn snap_flyout_bounds(win_rect: Rectangle<i32, Logical>) -> Rectangle<i32, Logical> {
    let flyout_w = 186;
    let flyout_h = 54;
    let fx = (win_rect.loc.x + win_rect.size.w - 194).max(win_rect.loc.x + 8);
    let fy = win_rect.loc.y + 34;
    Rectangle::new(Point::from((fx, fy)), Size::from((flyout_w, flyout_h)))
}

/// Returns `true` strictly when `cursor` is directly over the Maximize (`□`) titlebar button.
pub fn is_maximize_button_hovered(
    win_rect: Rectangle<i32, Logical>,
    cursor: Point<f64, Logical>,
) -> bool {
    if win_rect.size.w <= 160 {
        return false;
    }
    let rel_x = cursor.x - win_rect.loc.x as f64;
    let rel_y = cursor.y - win_rect.loc.y as f64;
    (4.0..=32.0).contains(&rel_y)
        && rel_x >= (win_rect.size.w as f64 - 58.0)
        && rel_x <= (win_rect.size.w as f64 - 34.0)
}

/// Returns `true` if `cursor` is hovering the Maximize (`□`) button or the Snap Layouts Flyout below it.
pub fn is_maximize_or_flyout_hovered(
    win_rect: Rectangle<i32, Logical>,
    cursor: Point<f64, Logical>,
) -> bool {
    if is_maximize_button_hovered(win_rect, cursor) {
        return true;
    }
    if win_rect.size.w <= 160 {
        return false;
    }
    let flyout = snap_flyout_bounds(win_rect);
    flyout.to_f64().contains(cursor)
}

/// Hit-tests the 6 miniature zone tiles inside the Snap Layouts Flyout card.
pub fn hit_test_snap_flyout(
    win_rect: Rectangle<i32, Logical>,
    cursor: Point<f64, Logical>,
) -> Option<SnapZone> {
    let bounds = snap_flyout_bounds(win_rect);
    if !bounds.to_f64().contains(cursor) {
        return None;
    }
    let local_x = (cursor.x - bounds.loc.x as f64).clamp(0.0, bounds.size.w as f64);
    let local_y = (cursor.y - bounds.loc.y as f64).clamp(0.0, bounds.size.h as f64);
    let col_w = bounds.size.w as f64 / 3.0;

    if local_x < col_w {
        // Group 1: 50/50 split
        if local_x < col_w * 0.5 {
            Some(SnapZone::LeftHalf)
        } else {
            Some(SnapZone::RightHalf)
        }
    } else if local_x < col_w * 2.0 {
        // Group 2: 2/3 + 1/3 split
        let in_col = local_x - col_w;
        if in_col < col_w * 0.62 {
            Some(SnapZone::LeftTwoThirds)
        } else {
            Some(SnapZone::RightOneThird)
        }
    } else {
        // Group 3: Quarter Grid (Top-Left vs Bottom-Right)
        if local_y < (bounds.size.h as f64 * 0.5) {
            Some(SnapZone::TopLeftQuarter)
        } else {
            Some(SnapZone::BottomRightQuarter)
        }
    }
}

/// Hit-tests interactive tab pills in the window titlebar when `tab_count >= 2`.
pub fn hit_test_titlebar_tab(
    win_rect: Rectangle<i32, Logical>,
    tab_count: usize,
    cursor: Point<f64, Logical>,
) -> Option<usize> {
    if tab_count < 2 || win_rect.size.w < 260 {
        return None;
    }
    let rel_x = cursor.x - win_rect.loc.x as f64;
    let rel_y = cursor.y - win_rect.loc.y as f64;
    if !(4.0..=32.0).contains(&rel_y) {
        return None;
    }
    let start_x = 148.0;
    let max_right = (win_rect.size.w as f64 - 96.0).max(start_x + 40.0);
    if rel_x < start_x || rel_x > max_right {
        return None;
    }
    let avail_w = max_right - start_x;
    let pill_w = (avail_w / tab_count as f64).clamp(36.0, 96.0);
    let idx = ((rel_x - start_x) / pill_w).floor() as usize;
    if idx < tab_count {
        Some(idx)
    } else {
        None
    }
}

/// Detects if a dragged window is hovered over the titlebar center zone of another window
/// to merge them into a Tabbed Window Stack.
pub fn detect_drag_tab_target(
    cursor: Point<f64, Logical>,
    active_layouts: &[(Window, Rectangle<i32, Logical>)],
    dragged: &Window,
) -> Option<(Window, Rectangle<i32, Logical>)> {
    let dragged_id = window_stable_id(dragged);
    for (win, rect) in active_layouts.iter().rev() {
        if window_stable_id(win) == dragged_id {
            continue;
        }
        let rel_x = cursor.x - rect.loc.x as f64;
        let rel_y = cursor.y - rect.loc.y as f64;
        let w = rect.size.w as f64;
        if (0.0..=36.0).contains(&rel_y) && rel_x >= (w * 0.16).max(36.0) && rel_x <= (w - 92.0).max(60.0) {
            return Some((win.clone(), *rect));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_mode_cycles_in_expected_order() {
        let mut manager = LayoutManager::new();
        assert_eq!(manager.mode(), LayoutMode::Floating);
        assert_eq!(manager.cycle_mode(), LayoutMode::MasterStack);
        assert_eq!(manager.cycle_mode(), LayoutMode::Dwindle);
        assert_eq!(manager.cycle_mode(), LayoutMode::Bsp);
        assert_eq!(manager.cycle_mode(), LayoutMode::Scrollable);
        assert_eq!(manager.cycle_mode(), LayoutMode::Monocle);
        assert_eq!(manager.cycle_mode(), LayoutMode::Floating);
    }

    #[test]
    fn test_per_workspace_modes_and_dynamic_workspaces() {
        let mut manager = LayoutManager::new();
        manager.set_workspace_mode(1, LayoutMode::Floating);
        manager.set_workspace_mode(2, LayoutMode::Bsp);
        manager.set_workspace_mode(3, LayoutMode::Scrollable);

        assert_eq!(manager.mode(), LayoutMode::Floating);
        manager.switch_workspace(2);
        assert_eq!(manager.mode(), LayoutMode::Bsp);
        manager.switch_workspace(3);
        assert_eq!(manager.mode(), LayoutMode::Scrollable);
        assert_eq!(manager.dynamic_workspace_count(), 4);
    }

    #[test]
    fn test_bsp_node_splits_and_computes_rects() {
        let mut root = BspNode::leaf(1);
        assert!(root.contains(1));
        assert!(!root.contains(2));

        // Insert window 2 split horizontally
        assert!(root.insert_adjacent(1, 2, SplitDirection::Horizontal, false));
        assert!(root.contains(1));
        assert!(root.contains(2));

        // Insert window 3 split vertically under window 2
        assert!(root.insert_adjacent(2, 3, SplitDirection::Vertical, false));
        assert!(root.contains(3));

        let bounds = Rectangle::new(Point::from((0, 0)), Size::from((1000, 800)));
        let gap = 10;
        let mut rects = HashMap::new();
        root.compute_rects(bounds, gap, &mut rects);

        // Total w = 1000 - 10 = 990. Split 50%: w_left = 495, w_right = 495
        let rect1 = rects.get(&1).unwrap();
        assert_eq!(rect1.loc.x, 0);
        assert_eq!(rect1.size.w, 495);
        assert_eq!(rect1.size.h, 800);

        let rect2 = rects.get(&2).unwrap();
        assert_eq!(rect2.loc.x, 495 + gap);
        assert_eq!(rect2.loc.y, 0);
        assert_eq!(rect2.size.w, 495);
        // Split vertically 800 - 10 = 790 -> 395 each
        assert_eq!(rect2.size.h, 395);

        let rect3 = rects.get(&3).unwrap();
        assert_eq!(rect3.loc.x, 495 + gap);
        assert_eq!(rect3.loc.y, 395 + gap);
        assert_eq!(rect3.size.w, 495);
        assert_eq!(rect3.size.h, 395);

        // Remove window 3 -> node collapses back to window 2 taking full right side
        root.remove(3);
        assert!(!root.contains(3));
        let mut collapsed_rects = HashMap::new();
        root.compute_rects(bounds, gap, &mut collapsed_rects);
        let rect2_collapsed = collapsed_rects.get(&2).unwrap();
        assert_eq!(rect2_collapsed.size.h, 800);
    }

    #[test]
    fn test_arrange_overview_empty_returns_empty() {
        let manager = LayoutManager::new();
        let area = Rectangle::new(Point::from((0, 0)), Size::from((1920, 1080)));
        assert!(manager.arrange_overview(area).is_empty());
    }

    #[test]
    fn test_snap_window_to_edges_and_magnetism() {
        let usable = Rectangle::new(Point::from((0, 36)), Size::from((1920, 1044)));
        let bounds = Rectangle::new(Point::from((0, 0)), Size::from((1920, 1080)));

        // 1. Left edge snap: window at x=8 (within 14px of 0) should snap to x=0
        let cand_left = Rectangle::new(Point::from((8, 100)), Size::from((600, 400)));
        let snapped_left = snap_window_to_edges(cand_left, usable, bounds, &[], 14);
        assert_eq!(snapped_left.loc.x, 0);

        // 2. Beyond threshold (x=25): should NOT snap
        let cand_free = Rectangle::new(Point::from((25, 100)), Size::from((600, 400)));
        let snapped_free = snap_window_to_edges(cand_free, usable, bounds, &[], 14);
        assert_eq!(snapped_free.loc.x, 25);

        // 3. Top edge (usable area at y=36, e.g. panel dock): window at y=42 should snap to y=36
        let cand_top = Rectangle::new(Point::from((200, 42)), Size::from((600, 400)));
        let snapped_top = snap_window_to_edges(cand_top, usable, bounds, &[], 14);
        assert_eq!(snapped_top.loc.y, 36);

        // 4. Neighbor snapping: window A at (100, 100, 400, 300) -> right edge is 500
        // Window B dragged to x=508 -> should snap to x=500
        let win_a = Rectangle::new(Point::from((100, 100)), Size::from((400, 300)));
        let win_b = Rectangle::new(Point::from((508, 120)), Size::from((350, 300)));
        let snapped_b = snap_window_to_edges(win_b, usable, bounds, &[win_a], 14);
        assert_eq!(snapped_b.loc.x, 500);

        // 5. Window B top alignment with Window A: y=107 should snap to y=100
        let win_b_top = Rectangle::new(Point::from((600, 107)), Size::from((350, 300)));
        let snapped_b_top = snap_window_to_edges(win_b_top, usable, bounds, &[win_a], 14);
        assert_eq!(snapped_b_top.loc.y, 100);
    }

    #[test]
    fn test_snap_window_resize_magnetism() {
        let usable = Rectangle::new(Point::from((0, 0)), Size::from((1920, 1080)));
        let bounds = usable;

        // Resizing right edge: loc.x=1000, width=913 -> cand_right = 1913
        // Screen right is 1920 (diff: 7px <= 14px) -> snaps width to 920 so right edge is 1920
        let cand_res = Rectangle::new(Point::from((1000, 200)), Size::from((913, 500)));
        let snapped_res = snap_window_resize(cand_res, usable, bounds, &[], 14);
        assert_eq!(snapped_res.size.w, 920);
        assert_eq!(snapped_res.loc.x + snapped_res.size.w, 1920);
    }

    #[test]
    fn test_clamp_floating_geometries_on_output_disconnect() {
        let mut layout = LayoutManager::new();
        let win_id = 42;
        // Window had geometry on a secondary monitor at (2500, 1400) with size 800x600
        let offscreen_rect = Rectangle::new(Point::from((2500, 1400)), Size::from((800, 600)));
        layout.floating_geometries.insert(win_id, offscreen_rect);

        // Secondary display disconnected: rescued to primary 1920x1044 usable area
        let primary_usable = Rectangle::new(Point::from((0, 36)), Size::from((1920, 1044)));
        layout.clamp_floating_geometries_to_area(primary_usable);

        let clamped = layout.floating_geometries.get(&win_id).unwrap();
        assert!(clamped.loc.x >= primary_usable.loc.x);
        assert!(clamped.loc.x + clamped.size.w <= primary_usable.loc.x + primary_usable.size.w);
        assert!(clamped.loc.y >= primary_usable.loc.y);
        assert!(clamped.loc.y + clamped.size.h <= primary_usable.loc.y + primary_usable.size.h);
    }

    #[test]
    fn test_window_target_alpha_and_blur_rules() {
        let rules = [
            WindowRuleConfig {
                app_id: Some("alacritty".to_string()),
                floating: Some(false),
                workspace: Some(1),
                opacity_percent: Some(85),
                blur: Some(true),
                ..Default::default()
            },
            WindowRuleConfig {
                app_id: Some("custom-app".to_string()),
                opacity_percent: Some(90),
                blur: Some(false),
                ..Default::default()
            },
        ];

        let rule_alacritty = &rules[0];
        assert_eq!(rule_alacritty.opacity_float(), Some(0.85));
        assert_eq!(rule_alacritty.blur, Some(true));

        let rule_custom = &rules[1];
        assert_eq!(rule_custom.opacity_float(), Some(0.90));
        assert_eq!(rule_custom.blur, Some(false));

        let mut manager = LayoutManager::new();
        // Target opacity defaults to 1.0
        let fake_wid = 999;
        assert_eq!(manager.target_alphas.get(&fake_wid).copied().unwrap_or(1.0), 1.0);
        manager.target_alphas.insert(fake_wid, 0.85);
        assert_eq!(manager.target_alphas.get(&fake_wid).copied().unwrap_or(1.0), 0.85);
    }

    #[test]
    fn test_titlebar_badge_extraction_and_dirty_dot() {
        let term = extract_titlebar_badge("Alacritty", "zsh — ~/Projects").unwrap();
        assert_eq!(term.label, "SHELL");
        assert!(!term.is_dirty);

        let dirty_dev = extract_titlebar_badge("zed", "• main.rs — Corten").unwrap();
        assert_eq!(dirty_dev.label, "DEV");
        assert!(dirty_dev.is_dirty);

        let audio = extract_titlebar_badge("spotify", "Synthwave Mix").unwrap();
        assert_eq!(audio.label, "AUDIO");
        assert!(!audio.is_dirty);
    }

    #[test]
    fn test_snap_layouts_flyout_hit_testing_and_zone_rects() {
        let usable = Rectangle::new(Point::from((0, 36)), Size::from((1920, 1044)));
        let left_half = compute_snap_zone_rect(usable, SnapZone::LeftHalf, 12);
        let right_half = compute_snap_zone_rect(usable, SnapZone::RightHalf, 12);
        assert_eq!(left_half.loc.x, 12);
        assert!(right_half.loc.x > left_half.loc.x + left_half.size.w);
        assert_eq!(left_half.size.h, right_half.size.h);

        let win_rect = Rectangle::new(Point::from((200, 100)), Size::from((800, 600)));
        let flyout = snap_flyout_bounds(win_rect);
        assert!(is_maximize_or_flyout_hovered(
            win_rect,
            Point::from((win_rect.loc.x as f64 + 755.0, win_rect.loc.y as f64 + 18.0))
        ));
        assert!(is_maximize_or_flyout_hovered(
            win_rect,
            Point::from((flyout.loc.x as f64 + 20.0, flyout.loc.y as f64 + 20.0))
        ));

        let z1 = hit_test_snap_flyout(
            win_rect,
            Point::from((flyout.loc.x as f64 + 10.0, flyout.loc.y as f64 + 20.0)),
        );
        assert_eq!(z1, Some(SnapZone::LeftHalf));

        let z2 = hit_test_snap_flyout(
            win_rect,
            Point::from((flyout.loc.x as f64 + 80.0, flyout.loc.y as f64 + 20.0)),
        );
        assert_eq!(z2, Some(SnapZone::LeftTwoThirds));

        let z3 = hit_test_snap_flyout(
            win_rect,
            Point::from((flyout.loc.x as f64 + 160.0, flyout.loc.y as f64 + 42.0)),
        );
        assert_eq!(z3, Some(SnapZone::BottomRightQuarter));
    }

    #[test]
    fn test_tabbed_window_stacking_and_tab_hit_testing() {
        let mut manager = LayoutManager::new();
        assert!(manager.merge_windows_into_tab_group(101, 202));
        assert!(manager.merge_windows_into_tab_group(101, 303));
        assert_eq!(manager.tab_group_for_window(202), Some(&[101, 202, 303][..]));

        let win_rect = Rectangle::new(Point::from((100, 100)), Size::from((800, 600)));
        let tab0 = hit_test_titlebar_tab(win_rect, 3, Point::from((265.0, 118.0)));
        assert_eq!(tab0, Some(0));

        assert!(manager.detach_window_from_tab_group(202));
        assert_eq!(manager.tab_group_for_window(101), Some(&[101, 303][..]));
        assert!(manager.detach_window_from_tab_group(303));
        assert_eq!(manager.tab_group_for_window(101), None);
    }

    #[test]
    fn test_8_directional_resize_edge_hit_testing() {
        let rect = Rectangle::new(Point::from((100, 100)), Size::from((800, 600)));
        assert_eq!(hit_test_resize_edge(rect, Point::from((102.0, 102.0)), 8.0), Some(ResizeEdge::TopLeft));
        assert_eq!(hit_test_resize_edge(rect, Point::from((898.0, 102.0)), 8.0), Some(ResizeEdge::TopRight));
        assert_eq!(hit_test_resize_edge(rect, Point::from((102.0, 698.0)), 8.0), Some(ResizeEdge::BottomLeft));
        assert_eq!(hit_test_resize_edge(rect, Point::from((898.0, 698.0)), 8.0), Some(ResizeEdge::BottomRight));
        assert_eq!(hit_test_resize_edge(rect, Point::from((102.0, 400.0)), 8.0), Some(ResizeEdge::Left));
        assert_eq!(hit_test_resize_edge(rect, Point::from((898.0, 400.0)), 8.0), Some(ResizeEdge::Right));
        assert_eq!(hit_test_resize_edge(rect, Point::from((500.0, 102.0)), 8.0), Some(ResizeEdge::Top));
        assert_eq!(hit_test_resize_edge(rect, Point::from((500.0, 698.0)), 8.0), Some(ResizeEdge::Bottom));
        assert_eq!(hit_test_resize_edge(rect, Point::from((500.0, 400.0)), 8.0), None);
    }

    #[test]
    fn test_compute_resized_rect_anchors_and_min_max_clamping() {
        let initial = Rectangle::new(Point::from((200, 150)), Size::from((600, 400)));

        // Left-edge drag left by 50px should grow width to 650 and move x to 150 (keeping right edge at 800)
        let left_grown = compute_resized_rect(
            initial,
            ResizeEdge::Left,
            Point::from((-50.0, 0.0)),
            (200, 140),
            (Some(700), Some(500)),
            0,
        );
        assert_eq!(left_grown.loc.x, 150);
        assert_eq!(left_grown.size.w, 650);
        assert_eq!(left_grown.loc.x + left_grown.size.w, 800);

        // TopLeft drag beyond max_size should clamp to max_size while keeping right/bottom anchor fixed
        let clamped_max = compute_resized_rect(
            initial,
            ResizeEdge::TopLeft,
            Point::from((-400.0, -400.0)),
            (200, 140),
            (Some(700), Some(500)),
            36,
        );
        assert_eq!(clamped_max.size.w, 700);
        assert_eq!(clamped_max.size.h, 536); // 500 client + 36 SSD titlebar
        assert_eq!(clamped_max.loc.x, 100); // 800 - 700
        assert_eq!(clamped_max.loc.y, 14);  // 550 - 536

        // Dragging right edge inward past min_size clamps cleanly at min_size
        let clamped_min = compute_resized_rect(
            initial,
            ResizeEdge::BottomRight,
            Point::from((-500.0, -500.0)),
            (250, 180),
            (None, None),
            0,
        );
        assert_eq!(clamped_min.size.w, 250);
        assert_eq!(clamped_min.size.h, 180);
    }
}
