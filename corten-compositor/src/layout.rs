use corten_config::{LayoutMode, WindowRuleConfig};
use serde::{Deserialize, Serialize};
use smithay::desktop::Window;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::Resource;
use smithay::utils::{Logical, Point, Rectangle, Size};
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

/// Configures a window's logical geometry for either Wayland or XWayland surfaces.
pub fn configure_window_geometry(window: &Window, rect: Rectangle<i32, Logical>) {
    if let Some(toplevel) = window.toplevel() {
        let needs_configure = toplevel.with_pending_state(|pending| {
            use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State;
            let is_fs = pending.states.contains(State::Fullscreen);
            let client_h = if is_fs { rect.size.h.max(100) } else { (rect.size.h - 36).max(100) };
            let new_size = Some((rect.size.w.max(160), client_h).into());
            let mut changed = false;
            if pending.size != new_size {
                pending.size = new_size;
                changed = true;
            }
            changed
        });
        if needs_configure {
            toplevel.send_configure();
        }
    } else if let Some(x11) = window.x11_surface() {
        let x11_rect = Rectangle::new(
            Point::from((rect.loc.x, rect.loc.y + 36)),
            Size::from((rect.size.w, (rect.size.h - 36).max(100))),
        );
        if x11.geometry() != x11_rect {
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

#[derive(Debug, Clone, Copy, Default)]
pub struct WindowRuleMatch {
    pub floating: Option<bool>,
    pub workspace: Option<usize>,
    pub fullscreen: Option<bool>,
    pub opacity: Option<f32>,
    pub blur: Option<bool>,
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
        self.mode
    }

    pub fn set_mode(&mut self, mode: LayoutMode) {
        self.mode = mode;
    }

    pub fn cycle_mode(&mut self) -> LayoutMode {
        self.mode = self.mode.next();
        self.mode
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
        let auto_float = should_auto_float(window);
        if app_id.is_empty() && title.is_empty() && !auto_float {
            return false;
        }
        self.evaluated_windows.insert(wid);

        let rule_match = evaluate_window_rules_full(window, rules);
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
        let current = std::mem::take(&mut self.windows);
        let floating = std::mem::take(&mut self.floating_windows);
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
        self.workspace = workspace;
        self.active_index = self.active_index.min(self.windows.len().saturating_sub(1));
        self.active_floating = None;
        self.animated_rects.clear();
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
        if maximized {
            if let Some(&current_geo) = self.floating_geometries.get(&wid) {
                self.saved_unmaximized_geometries.insert(wid, current_geo);
            }
            self.maximized_windows.insert(wid);
            self.set_floating_geometry(window, usable_area);
            configure_window_geometry(window, usable_area);
            if let Some(toplevel) = window.toplevel() {
                toplevel.with_pending_state(|tstate| {
                    use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State;
                    tstate.states.set(State::Maximized);
                    tstate.states.unset(State::Fullscreen);
                });
                toplevel.send_configure();
            }
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
            configure_window_geometry(window, restored);
            if let Some(toplevel) = window.toplevel() {
                toplevel.with_pending_state(|tstate| {
                    use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State;
                    tstate.states.unset(State::Maximized);
                });
                toplevel.send_configure();
            }
            restored
        }
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

    /// Set or update the position/size of a floating window
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
            let rect = match self.mode {
                LayoutMode::Floating => {
                    if let Some(&saved) = self.floating_geometries.get(&win_id) {
                        saved
                    } else {
                        let win_geo = win.geometry();
                        let (w, h) = if win_geo.size.w > 0 && win_geo.size.h > 0 {
                            (win_geo.size.w, win_geo.size.h + 36)
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
                LayoutMode::MasterStack | LayoutMode::Dwindle | LayoutMode::Bsp => Rectangle::new(
                    Point::from((total_x, total_y)),
                    Size::from((total_w, total_h)),
                ),
            };
            results.push((win.clone(), rect));
            return results;
        }

        match self.mode {
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

        let spring_stiffness = 0.32; // Responsive 60-120fps smooth spring dampening factor
        let alpha_stiffness = 0.28_f32;

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
                // Initial spawn animation: start slightly smaller (92% scale) and centered
                let spawn_inset_x = target_fw * 0.04;
                let spawn_inset_y = target_fh * 0.04;
                Rectangle::new(
                    Point::from((target_fx + spawn_inset_x, target_fy + spawn_inset_y)),
                    Size::from((target_fw * 0.92, target_fh * 0.92)),
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
                        current_entry.loc.x.round() as i32,
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

        // Include floating windows centered or at their saved geometries
        for (i, window) in self.floating_windows.iter().enumerate() {
            let win_id = window_stable_id(window);
            let target_alpha = self.window_target_alpha(window);
            let alpha_entry = self.animated_alphas.entry(win_id).or_insert(0.15);
            *alpha_entry += (target_alpha - *alpha_entry) * alpha_stiffness;
            if (target_alpha - *alpha_entry).abs() < 0.015 {
                *alpha_entry = target_alpha;
            }
            let float_rect = if self.is_fullscreen(window) {
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
            animated_results.push((window.clone(), float_rect));
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
        assert_eq!(manager.cycle_mode(), LayoutMode::Monocle);
        assert_eq!(manager.cycle_mode(), LayoutMode::Floating);
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
                title: None,
                floating: Some(false),
                workspace: Some(1),
                fullscreen: None,
                opacity_percent: Some(85),
                blur: Some(true),
            },
            WindowRuleConfig {
                app_id: Some("custom-app".to_string()),
                title: None,
                floating: None,
                workspace: None,
                fullscreen: None,
                opacity_percent: Some(90),
                blur: Some(false),
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
}
