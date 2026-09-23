//! Hallmark · component: agent activity · design-system: design.md.
//! Pre-emit critique: P5 H5 E4 S5 R5 V4. Content-sized; the host owns notch chrome.
use crate::{
    connections::{Connections, Installation},
    model::{Provider, Session, Status},
};
use fontdue::{Font, FontSettings};
use island_design::{PILL_BG, TEXT_PRIMARY, TEXT_SECONDARY, agent_pulse as t, market};
use island_plugin_api::SurfaceFrame;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Menu,
    Back,
    Setup,
    Copy,
    Open,
    Settings,
    Row(usize),
    Select(usize),
    Scrollbar,
}
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    #[default]
    Sessions,
    Menu,
    Settings,
}
#[derive(Default)]
pub struct Ui {
    pub list: crate::list::Viewport,
    pub selected: usize,
    pub hover: Option<Action>,
    pub pressed: Option<Action>,
    pub focus: Option<Action>,
    pub connections: Connections,
    pub loading: bool,
    pub message: String,
    pub error: bool,
    pub setup_error: bool,
    pub page: Page,
    pub expanded: bool,
    pub codex_available: bool,
}

impl Ui {
    pub fn reconcile_sessions(&mut self, old: &[Session], new: &[Session], h: u32) {
        let same_order = old.len() == new.len()
            && old
                .iter()
                .zip(new)
                .all(|(a, b)| a.id == b.id && a.provider == b.provider);
        if !same_order {
            if let Some(Action::Row(index)) = self.focus {
                self.focus = old
                    .get(index)
                    .and_then(|focused| {
                        new.iter()
                            .position(|s| s.id == focused.id && s.provider == focused.provider)
                    })
                    .map(Action::Row);
            }
            // Do not allow a pressed row to turn into a different session while
            // hooks reorder the list. Keep the viewport anchored by identity.
            self.pressed = None;
            self.hover = None;
            self.list.end_drag();
            if self.list.offset > 0.0 {
                let top = (self.list.offset / t::ROW).floor() as usize;
                if let Some(anchor) = old.get(top)
                    && let Some(index) = new
                        .iter()
                        .position(|s| s.id == anchor.id && s.provider == anchor.provider)
                {
                    self.list.offset = index as f32 * t::ROW + self.list.offset % t::ROW;
                }
            }
        }
        self.list.clamp(new.len(), h);
    }
}

pub fn preferred_height(count: usize, page: Page) -> u32 {
    match page {
        Page::Menu => t::MENU_HEIGHT,
        Page::Settings => t::SETTINGS_HEIGHT,
        Page::Sessions if count == 0 => 132,
        Page::Sessions => {
            (t::HEADER + (count as f32 * t::ROW).min(t::LIST_VIEWPORT_HEIGHT) + t::BOTTOM) as u32
        }
    }
}
/// A UUID-only route cannot inject a URL, command, or another app action.
pub use crate::navigation::codex_url;
pub fn actions(sessions: &[Session], ui: &Ui) -> Vec<Action> {
    match ui.page {
        Page::Sessions => {
            let mut actions = vec![Action::Menu];
            if sessions.is_empty() {
                actions.push(Action::Setup);
            }
            actions
        }
        Page::Menu => {
            let mut actions = vec![Action::Back];
            if sessions.get(ui.selected).is_some_and(|s| {
                !matches!(
                    crate::navigation::target(s, ui.codex_available),
                    crate::navigation::Target::Unavailable
                )
            }) {
                actions.push(Action::Open);
            }
            if !sessions.is_empty() {
                actions.push(Action::Copy);
            }
            actions.push(Action::Settings);
            actions
        }
        Page::Settings if ui.loading => vec![Action::Back],
        Page::Settings => vec![Action::Back, Action::Setup],
    }
}
pub fn hit(w: u32, h: u32, sessions: &[Session], ui: &Ui, x: f32, y: f32) -> Option<Action> {
    if !x.is_finite() || !y.is_finite() || x < 0.0 || y < 0.0 || x >= w as f32 || y >= h as f32 {
        return None;
    }
    if ui.page != Page::Sessions && x < 40.0 && y < t::HEADER {
        return Some(Action::Back);
    }
    match ui.page {
        Page::Sessions => {
            if x >= w as f32 - 42.0 && y < t::HEADER {
                return Some(Action::Menu);
            }
            if sessions.is_empty() {
                return ((92.0..124.0).contains(&y) && (t::INSET..160.0).contains(&x))
                    .then_some(Action::Setup);
            }
            if h >= 64 && crate::list::Viewport::contains_y(h, y) {
                if x >= w as f32 - t::SCROLLBAR_HIT_WIDTH
                    && ui.list.thumb(sessions.len(), h).is_some()
                {
                    return Some(Action::Scrollbar);
                }
                return ui.list.hit(sessions.len(), h, y).map(Action::Row);
            }
        }
        Page::Menu => {
            if y >= 36.0 {
                return actions(sessions, ui)
                    .into_iter()
                    .skip(1)
                    .nth(((y - 36.0) / 42.0) as usize);
            }
        }
        Page::Settings => {
            if (t::CONNECTION_ACTION_Y..t::CONNECTION_ACTION_Y + 32.0).contains(&y)
                && (t::INSET..166.0).contains(&x)
                && !ui.loading
            {
                return Some(Action::Setup);
            }
        }
    }
    None
}

pub struct Renderer {
    font: Option<Font>,
    icons: [image::RgbaImage; 3],
}
impl Default for Renderer {
    fn default() -> Self {
        Self::new()
    }
}
impl Renderer {
    pub fn new() -> Self {
        let mut paths = vec![
            "/System/Library/Fonts/HelveticaNeue.ttc".into(),
            "/System/Library/Fonts/Supplemental/Arial.ttf".into(),
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf".into(),
            "/usr/share/fonts/TTF/DejaVuSans.ttf".into(),
            "/usr/share/fonts/truetype/liberation2/LiberationSans-Regular.ttf".into(),
        ];
        if let Some(win) = std::env::var_os("WINDIR") {
            paths.push(std::path::PathBuf::from(win).join("Fonts/segoeui.ttf"));
        }
        let font = paths.iter().find_map(|p| {
            std::fs::read(p)
                .ok()
                .and_then(|b| Font::from_bytes(b, FontSettings::default()).ok())
        });
        let icons = [
            include_bytes!("../assets/claude.png").as_slice(),
            include_bytes!("../assets/codex.png").as_slice(),
            include_bytes!("../assets/grok.png").as_slice(),
        ]
        .map(|bytes| {
            image::load_from_memory(bytes)
                .expect("bundled provider icon")
                .into_rgba8()
        });
        Self { font, icons }
    }
    pub fn has_font(&self) -> bool {
        cfg!(target_os = "macos") || self.font.is_some()
    }
    fn icon(&self, provider: Provider) -> &image::RgbaImage {
        &self.icons[match provider {
            Provider::Claude => 0,
            Provider::Codex => 1,
            Provider::Grok => 2,
        }]
    }
    pub fn collapsed_icon(&self, provider: Provider) -> SurfaceFrame {
        let mut canvas = Canvas::new(24, 24, 3.0, None);
        canvas.rect(0.0, 0.0, 24.0, 24.0, PILL_BG);
        canvas.image(1.0, 1.0, 22.0, self.icon(provider));
        canvas.frame
    }
    pub fn frame(
        &self,
        w: u32,
        h: u32,
        scale: f32,
        sessions: &[Session],
        ui: &Ui,
        now: u64,
    ) -> SurfaceFrame {
        let w = w.clamp(80, 512);
        let h = h.clamp(20, 256);
        let scale = if scale.is_finite() {
            scale.clamp(1.0, 3.0)
        } else {
            1.0
        };
        let mut c = Canvas::new(w, h, scale, self.font.as_ref());
        c.rect(0.0, 0.0, w as f32, h as f32, PILL_BG);
        if h < 64 {
            let s = sessions.first();
            if let Some(s) = s {
                c.image(10.0, 3.0, 22.0, self.icon(s.provider));
            }
            c.text(
                42.0,
                5.0,
                &summary(sessions),
                12.0,
                TEXT_PRIMARY,
                w as f32 - 68.0,
            );
            indicator(
                &mut c,
                w as f32 - 14.0,
                h as f32 / 2.0,
                s.map(|s| s.status).unwrap_or(Status::Idle),
                now,
            );
            return c.frame;
        }
        if ui.page == Page::Sessions {
            let title = if !ui.message.is_empty() {
                ui.message.clone()
            } else if sessions.iter().any(Session::needs_attention) {
                format!("Agent Pulse  ·  {}", summary(sessions))
            } else if sessions.len() > 1 {
                format!("Agent Pulse  ·  {} sessions", sessions.len())
            } else {
                "Agent Pulse".into()
            };
            c.text(
                t::INSET,
                6.0,
                &title,
                t::LABEL,
                if ui.error && !ui.message.is_empty() {
                    t::ERROR
                } else {
                    TEXT_SECONDARY
                },
                w as f32 - 64.0,
            );
            control_bg(&mut c, ui, Action::Menu, w as f32 - 39.0, 1.0, 30.0, 25.0);
            for i in 0..3 {
                c.circle(w as f32 - 29.0 + i as f32 * 5.0, 13.0, 1.1, TEXT_SECONDARY);
            }
            if sessions.is_empty() {
                c.text(
                    t::INSET,
                    39.0,
                    if ui.connections.configured() {
                        "Ready when you are"
                    } else {
                        "Your agents, at a glance"
                    },
                    t::TITLE,
                    TEXT_PRIMARY,
                    w as f32 - 28.0,
                );
                c.text(
                    t::INSET,
                    63.0,
                    if ui.connections.configured() {
                        "Start a task. Status will appear here."
                    } else {
                        "Claude Code, Codex and Grok."
                    },
                    t::LABEL,
                    TEXT_SECONDARY,
                    w as f32 - 28.0,
                );
                button(
                    &mut c,
                    ui,
                    Action::Setup,
                    (t::INSET, 92.0, 146.0),
                    if ui.loading {
                        "Connecting…"
                    } else if ui.connections.configured() {
                        "Connection settings"
                    } else {
                        "Connect agents"
                    },
                );
            } else {
                c.clip_y = (
                    (t::HEADER * scale).round() as i32,
                    ((h as f32 - t::BOTTOM) * scale).round() as i32,
                );
                for index in ui.list.rows(sessions.len(), h) {
                    let s = &sessions[index];
                    let y = ui.list.row_y(index, sessions.len(), h);
                    let action = Action::Row(index);
                    if sessions.len() > 1 && ui.selected == index {
                        c.rounded(5.0, y, w as f32 - 10.0, t::ROW - 2.0, 9.0, t::SURFACE);
                    }
                    control_bg(&mut c, ui, action, 5.0, y, w as f32 - 10.0, t::ROW - 2.0);
                    c.rounded(t::INSET, y + 10.0, t::ICON, t::ICON, 8.0, t::SURFACE);
                    c.image(t::INSET, y + 10.0, t::ICON, self.icon(s.provider));
                    let title = if s.project.is_empty() {
                        "Session"
                    } else {
                        &s.project
                    };
                    c.text(
                        54.0,
                        y + 6.0,
                        title,
                        t::TITLE,
                        TEXT_PRIMARY,
                        w as f32 - 94.0,
                    );
                    let detail = s.activity_label();
                    c.text(
                        54.0,
                        y + 28.0,
                        &format!("{} · {detail}", s.provider.name()),
                        t::LABEL,
                        if s.needs_attention() {
                            t::WAITING
                        } else {
                            TEXT_SECONDARY
                        },
                        w as f32 - 80.0,
                    );
                    indicator(&mut c, w as f32 - 22.0, y + 21.0, s.status, now);
                }
                c.clip_y = (0, c.frame.height as i32);
                if let Some(thumb) = ui.list.thumb(sessions.len(), h) {
                    c.rounded(
                        w as f32 - 5.0,
                        thumb.y,
                        t::SCROLLBAR_WIDTH,
                        thumb.height,
                        t::SCROLLBAR_WIDTH / 2.0,
                        t::SCROLLBAR,
                    );
                }
            }
        } else {
            control_bg(&mut c, ui, Action::Back, 7.0, 1.0, 28.0, 25.0);
            c.line((23.0, 8.0), (18.0, 13.0), 1.4, TEXT_SECONDARY);
            c.line((18.0, 13.0), (23.0, 18.0), 1.4, TEXT_SECONDARY);
            c.text(
                43.0,
                6.0,
                if ui.page == Page::Settings {
                    "Connections"
                } else if ui.error && !ui.message.is_empty() {
                    &ui.message
                } else {
                    "Session actions"
                },
                t::LABEL,
                TEXT_SECONDARY,
                w as f32 - 60.0,
            );
            if ui.page == Page::Menu {
                let open_label = match sessions
                    .get(ui.selected)
                    .map(|s| crate::navigation::target(s, ui.codex_available))
                {
                    Some(crate::navigation::Target::Application(origin)) => {
                        format!("Open {}", origin.name)
                    }
                    Some(crate::navigation::Target::Url(_)) => "Open in Codex".into(),
                    _ => "Open session".into(),
                };
                for (i, action) in actions(sessions, ui).into_iter().skip(1).enumerate() {
                    let y = 36.0 + i as f32 * 42.0;
                    control_bg(&mut c, ui, action, 8.0, y, w as f32 - 16.0, 38.0);
                    let label = match action {
                        Action::Open => &open_label,
                        Action::Copy if !ui.message.is_empty() && !ui.error => &ui.message,
                        Action::Copy => "Copy resume command",
                        _ => "Connection settings",
                    };
                    c.text(
                        18.0,
                        y + 10.0,
                        label,
                        13.0,
                        if ui.error && action == Action::Copy {
                            t::ERROR
                        } else {
                            TEXT_PRIMARY
                        },
                        w as f32 - 52.0,
                    );
                    c.text(w as f32 - 30.0, y + 10.0, "›", 14.0, TEXT_SECONDARY, 15.0);
                }
            } else {
                for (index, provider) in Provider::ALL.into_iter().enumerate() {
                    let y = t::HEADER + index as f32 * t::CONNECTION_ROW;
                    c.rounded(t::INSET, y + 5.0, t::ICON, t::ICON, 8.0, t::SURFACE);
                    c.image(t::INSET, y + 5.0, t::ICON, self.icon(provider));
                    c.text(54.0, y + 2.0, provider.name(), 13.0, TEXT_PRIMARY, 142.0);
                    let setup = ui.connections.installation[index];
                    c.text(
                        w as f32 - 137.0,
                        y + 3.0,
                        setup.label(),
                        t::LABEL,
                        if setup == Installation::Incomplete {
                            t::WAITING
                        } else {
                            TEXT_SECONDARY
                        },
                        123.0,
                    );
                    c.text(
                        54.0,
                        y + 22.0,
                        &ui.connections.activity_label(index, now),
                        t::LABEL,
                        if ui.connections.recent(index, now) {
                            t::SUCCESS
                        } else {
                            TEXT_SECONDARY
                        },
                        w as f32 - 82.0,
                    );
                }
                let hint = if ui.loading {
                    ["Installing local hooks…", "Waiting for setup to finish."]
                } else if ui.setup_error {
                    [
                        "Setup failed; existing hooks may still work.",
                        "Try again or check the Island log.",
                    ]
                } else {
                    ui.connections.hint(now)
                };
                for (line, text) in hint.into_iter().enumerate() {
                    c.text(
                        t::INSET,
                        t::CONNECTION_HINT_Y + line as f32 * 15.0,
                        text,
                        t::LABEL,
                        if ui.setup_error {
                            t::ERROR
                        } else {
                            TEXT_SECONDARY
                        },
                        w as f32 - 28.0,
                    );
                }
                button(
                    &mut c,
                    ui,
                    Action::Setup,
                    (t::INSET, t::CONNECTION_ACTION_Y, 152.0),
                    if ui.loading {
                        "Installing…"
                    } else {
                        ui.connections.action_label()
                    },
                );
            }
        }
        c.frame
    }
}
pub fn summary(sessions: &[Session]) -> String {
    let waiting = sessions.iter().filter(|s| s.needs_attention()).count();
    let checking = sessions.iter().filter(|s| s.checking_permissions()).count();
    let working = sessions
        .iter()
        .filter(|s| s.status == Status::Working && !s.checking_permissions())
        .count();
    if waiting > 0 {
        if waiting == 1 {
            "1 needs you".into()
        } else {
            format!("{waiting} need you")
        }
    } else if working > 0 {
        format!("{working} working")
    } else if checking > 0 {
        "Checking permissions".into()
    } else {
        sessions
            .first()
            .map(|s| s.status.label())
            .unwrap_or("All quiet")
            .into()
    }
}
fn control_bg(c: &mut Canvas<'_>, ui: &Ui, action: Action, x: f32, y: f32, w: f32, h: f32) {
    if ui.focus == Some(action) {
        c.rounded(x, y, w, h, 8.0, market::FOCUS);
        c.rounded(x + 1.5, y + 1.5, w - 3.0, h - 3.0, 6.5, PILL_BG);
    }
    if ui.pressed == Some(action) {
        c.rounded(x + 2.0, y + 2.0, w - 4.0, h - 4.0, 6.0, t::PRESSED);
    } else if ui.hover == Some(action) {
        c.rounded(x + 2.0, y + 2.0, w - 4.0, h - 4.0, 6.0, t::HOVER);
    }
}
fn button(c: &mut Canvas<'_>, ui: &Ui, action: Action, bounds: (f32, f32, f32), label: &str) {
    let (x, y, w) = bounds;
    c.rounded(x, y, w, 32.0, 8.0, t::SURFACE);
    control_bg(c, ui, action, x, y, w, 32.0);
    c.text(
        x + 12.0,
        y + 8.0,
        label,
        12.0,
        if ui.loading {
            TEXT_SECONDARY
        } else {
            TEXT_PRIMARY
        },
        w - 24.0,
    );
}
fn indicator(c: &mut Canvas<'_>, x: f32, y: f32, status: Status, now: u64) {
    match status {
        Status::Working => {
            for i in 0..8 {
                let a = i as f32 * std::f32::consts::TAU / 8.0;
                let bright = (i + 8 - (now / 120) % 8) % 8 < 3;
                c.circle(
                    x + a.cos() * 5.0,
                    y + a.sin() * 5.0,
                    1.0,
                    if bright { TEXT_PRIMARY } else { t::PRESSED },
                );
            }
        }
        Status::Waiting | Status::Error => {
            let ink = if status == Status::Waiting {
                t::WAITING
            } else {
                t::ERROR
            };
            c.circle(x, y, 7.0, ink);
            c.line((x, y - 3.0), (x, y), 1.4, PILL_BG);
            c.circle(x, y + 3.0, 0.8, PILL_BG);
        }
        Status::Ready => {
            c.line((x - 4.0, y), (x - 1.0, y + 3.0), 1.5, t::SUCCESS);
            c.line((x - 1.0, y + 3.0), (x + 5.0, y - 4.0), 1.5, t::SUCCESS);
        }
        _ => c.circle(x, y, 2.0, TEXT_SECONDARY),
    }
}
struct Canvas<'a> {
    frame: SurfaceFrame,
    scale: f32,
    clip_y: (i32, i32),
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    font: Option<&'a Font>,
}
impl<'a> Canvas<'a> {
    fn image(&mut self, x: f32, y: f32, size: f32, source: &image::RgbaImage) {
        let side = (size * self.scale).round() as u32;
        let icon =
            image::imageops::resize(source, side, side, image::imageops::FilterType::Lanczos3);
        for (ix, iy, rgba) in icon.enumerate_pixels() {
            let [r, g, b, alpha] = rgba.0;
            self.pixel(
                (x * self.scale) as i32 + ix as i32,
                (y * self.scale) as i32 + iy as i32,
                (r as u32) << 16 | (g as u32) << 8 | b as u32,
                alpha,
            );
        }
    }
    fn new(w: u32, h: u32, scale: f32, font: Option<&'a Font>) -> Self {
        let width = (w as f32 * scale).round() as u32;
        let height = (h as f32 * scale).round() as u32;
        Self {
            frame: SurfaceFrame {
                width,
                height,
                frame_id: 0,
                pixels: vec![0; (width * height * 4) as usize],
            },
            scale,
            clip_y: (0, height as i32),
            font,
        }
    }
    fn pixel(&mut self, x: i32, y: i32, color: u32, alpha: u8) {
        if x < 0
            || y < self.clip_y.0
            || y >= self.clip_y.1
            || x >= self.frame.width as i32
            || y >= self.frame.height as i32
        {
            return;
        }
        let i = ((y as u32 * self.frame.width + x as u32) * 4) as usize;
        for (channel, shift) in [0, 8, 16].into_iter().enumerate() {
            let old = self.frame.pixels[i + channel] as u32;
            self.frame.pixels[i + channel] = ((((color >> shift) & 255) * alpha as u32
                + old * (255 - alpha as u32))
                / 255) as u8;
        }
        self.frame.pixels[i + 3] = 255;
    }
    fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: u32) {
        for yy in (y * self.scale) as i32..((y + h) * self.scale) as i32 {
            for xx in (x * self.scale) as i32..((x + w) * self.scale) as i32 {
                self.pixel(xx, yy, color, 255);
            }
        }
    }
    fn circle(&mut self, x: f32, y: f32, radius: f32, color: u32) {
        let r = radius * self.scale;
        let (cx, cy) = (x * self.scale, y * self.scale);
        for yy in (cy - r - 1.0) as i32..=(cy + r + 1.0) as i32 {
            for xx in (cx - r - 1.0) as i32..=(cx + r + 1.0) as i32 {
                let alpha = ((r - ((xx as f32 - cx).powi(2) + (yy as f32 - cy).powi(2)).sqrt())
                    .clamp(0.0, 1.0)
                    * 255.0) as u8;
                self.pixel(xx, yy, color, alpha);
            }
        }
    }
    fn rounded(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, color: u32) {
        for py in (y * self.scale).floor() as i32..((y + h) * self.scale).ceil() as i32 {
            for px in (x * self.scale).floor() as i32..((x + w) * self.scale).ceil() as i32 {
                let lx = (px as f32 + 0.5) / self.scale;
                let ly = (py as f32 + 0.5) / self.scale;
                let dx = (lx - (x + w / 2.0)).abs() - (w / 2.0 - r);
                let dy = (ly - (y + h / 2.0)).abs() - (h / 2.0 - r);
                let d = dx.max(0.0).hypot(dy.max(0.0)) + dx.max(dy).min(0.0) - r;
                self.pixel(
                    px,
                    py,
                    color,
                    ((0.5 - d * self.scale).clamp(0.0, 1.0) * 255.0) as u8,
                );
            }
        }
    }
    fn line(&mut self, a: (f32, f32), b: (f32, f32), width: f32, color: u32) {
        let steps = ((b.0 - a.0).hypot(b.1 - a.1) * self.scale * 2.0).ceil() as usize;
        for i in 0..=steps {
            let f = i as f32 / steps.max(1) as f32;
            self.circle(
                a.0 + (b.0 - a.0) * f,
                a.1 + (b.1 - a.1) * f,
                width / 2.0 + 0.4 / self.scale,
                color,
            );
        }
    }
    fn text(&mut self, x: f32, y: f32, text: &str, size: f32, color: u32, max_width: f32) {
        #[cfg(target_os = "macos")]
        {
            let mask = crate::native_text::rasterize(
                text,
                size * self.scale,
                max_width.max(1.0) * self.scale,
                size >= t::TITLE,
            );
            for row in 0..mask.height {
                for col in 0..mask.width {
                    self.pixel(
                        (x * self.scale) as i32 + col as i32,
                        (y * self.scale) as i32 + row as i32,
                        color,
                        mask.alpha[row * mask.width + col],
                    );
                }
            }
        }
        #[cfg(not(target_os = "macos"))]
        self.portable_text(x, y, text, size, color, max_width);
    }
    #[cfg(not(target_os = "macos"))]
    fn portable_text(&mut self, x: f32, y: f32, text: &str, size: f32, color: u32, max_width: f32) {
        let Some(font) = self.font else {
            return;
        };
        let size = size * self.scale;
        let mut cursor = x * self.scale;
        let end = cursor + max_width.max(0.0) * self.scale;
        let chars: Vec<char> = text.chars().collect();
        for (index, &ch) in chars.iter().enumerate() {
            let next = font.metrics(ch, size).advance_width;
            let ellipsis = font.metrics('…', size).advance_width;
            let trunc = index + 1 < chars.len() && cursor + next + ellipsis > end;
            let (m, bitmap) = font.rasterize(if trunc { '…' } else { ch }, size);
            for row in 0..m.height {
                for col in 0..m.width {
                    let px = cursor as i32 + m.xmin + col as i32;
                    if px < end as i32 {
                        self.pixel(
                            px,
                            (y * self.scale + size) as i32 - m.height as i32 - m.ymin + row as i32,
                            color,
                            bitmap[row * m.width + col],
                        );
                    }
                }
            }
            if trunc {
                break;
            }
            cursor += m.advance_width;
        }
    }
}

pub fn samples(now: u64) -> Vec<Session> {
    [
        (
            Provider::Claude,
            "island",
            Status::Waiting,
            "Permission requested",
        ),
        (
            Provider::Codex,
            "companion",
            Status::Working,
            "Running tests",
        ),
        (Provider::Grok, "plugin-sdk", Status::Ready, "Turn ended"),
    ]
    .into_iter()
    .enumerate()
    .map(|(i, (provider, project, status, detail))| Session {
        provider,
        id: format!("sample-{i}"),
        project: project.into(),
        status,
        detail: detail.into(),
        turn_id: None,
        turn_started_ms: now - 90_000,
        updated_ms: now - (i as u64 + 1) * 12_000,
        signal_ms: now - 1000,
        last_working_ms: 0,
        origin: None,
        terminal: None,
        pending_requests: vec![],
    })
    .collect()
}

pub fn export(path: &std::path::Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(path)?;
    let renderer = Renderer::new();
    anyhow::ensure!(
        renderer.has_font(),
        "Install a system font before exporting previews"
    );
    let working = Session {
        provider: Provider::Grok,
        project: "vibe-coders".into(),
        status: Status::Working,
        detail: "Thinking".into(),
        ..samples(100_000).remove(1)
    };
    let mut reviewing = crate::model::Sessions::default();
    let mut checking = vec![];
    for (at, event) in [(99_000, "PermissionRequest"), (99_001, "PreToolUse")] {
        reviewing.apply(
            crate::hooks::normalize(
                Provider::Codex,
                &serde_json::json!({
                    "session_id": "preview", "turn_id": "preview", "cwd": "/sample/build-check",
                    "hook_event_name": event, "tool_name": "Bash",
                    "tool_input": { "command": "fixture" }
                }),
                at,
            )
            .unwrap(),
        );
        if event == "PermissionRequest" {
            checking = reviewing.sessions.clone();
        }
    }
    let base = || Ui {
        connections: Connections {
            installation: [Installation::Installed; 3],
            last_signal_ms: [None, Some(99_000), Some(40_000)],
            ..Connections::default()
        },
        ..Ui::default()
    };
    let mut states = vec![
        ("compact", 245, 24, samples(100_000), base()),
        (
            "expanded",
            t::WIDTH,
            preferred_height(3, Page::Sessions),
            samples(100_000),
            base(),
        ),
        (
            "working",
            t::WIDTH,
            preferred_height(1, Page::Sessions),
            vec![working.clone()],
            base(),
        ),
        (
            "attention",
            t::WIDTH,
            preferred_height(1, Page::Sessions),
            vec![samples(100_000).remove(0)],
            base(),
        ),
        (
            "working-after-review",
            t::WIDTH,
            preferred_height(1, Page::Sessions),
            reviewing.sessions,
            base(),
        ),
        (
            "checking-permissions",
            t::WIDTH,
            preferred_height(1, Page::Sessions),
            checking,
            base(),
        ),
        (
            "complete",
            t::WIDTH,
            preferred_height(1, Page::Sessions),
            vec![samples(100_000).remove(2)],
            base(),
        ),
        (
            "empty",
            t::WIDTH,
            preferred_height(0, Page::Sessions),
            vec![],
            Ui::default(),
        ),
        (
            "settings",
            t::WIDTH,
            t::SETTINGS_HEIGHT,
            vec![working.clone()],
            Ui {
                page: Page::Settings,
                ..base()
            },
        ),
        (
            "menu",
            t::WIDTH,
            t::MENU_HEIGHT,
            vec![working.clone()],
            Ui {
                page: Page::Menu,
                ..base()
            },
        ),
    ];
    let many: Vec<_> = [
        "island",
        "companion",
        "plugin-sdk",
        "agent-pulse",
        "music-library",
        "renderer",
        "docs",
        "release-tools",
    ]
    .into_iter()
    .enumerate()
    .map(|(i, name)| {
        let mut session = samples(100_000).remove(if i == 0 { 0 } else { 1 + i % 2 });
        session.id = format!("scroll-sample-{i}");
        session.project = name.into();
        session
    })
    .collect();
    states.push((
        "scroll-top",
        t::WIDTH,
        preferred_height(many.len(), Page::Sessions),
        many.clone(),
        base(),
    ));
    let mut scrolled = base();
    scrolled.list.offset = t::ROW * 3.5;
    states.push((
        "scroll-middle",
        t::WIDTH,
        preferred_height(many.len(), Page::Sessions),
        many,
        scrolled,
    ));
    for (name, hover, pressed, focus, loading, error, message) in [
        ("hover", Some(Action::Menu), None, None, false, false, ""),
        ("focus", None, None, Some(Action::Menu), false, false, ""),
        ("pressed", None, Some(Action::Menu), None, false, false, ""),
        ("loading", None, None, None, true, false, ""),
        ("error", None, None, None, false, true, "Copy failed"),
        ("success", None, None, None, false, false, "Copied"),
    ] {
        let page = if loading {
            Page::Settings
        } else if !message.is_empty() {
            Page::Menu
        } else {
            Page::Sessions
        };
        states.push((
            name,
            t::WIDTH,
            preferred_height(1, page),
            vec![working.clone()],
            Ui {
                hover,
                pressed,
                focus,
                loading,
                error,
                message: message.into(),
                page,
                ..base()
            },
        ));
    }
    for (name, w, h, sessions, ui) in states {
        let frame = renderer.frame(w, h, 2.0, &sessions, &ui, 100_000);
        let mut rgba = frame.pixels;
        for pixel in rgba.as_chunks_mut::<4>().0.iter_mut() {
            pixel.swap(0, 2);
        }
        image::save_buffer(
            path.join(format!("{name}.png")),
            &rgba,
            frame.width,
            frame.height,
            image::ColorType::Rgba8,
        )?;
    }
    Ok(())
}
