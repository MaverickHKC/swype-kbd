//! swype-kbd — milestone 1.
//!
//! A `zwlr_layer_shell_v1` client that docks a software-rendered QWERTY keyboard
//! to the bottom of the screen and prints the key hit by each pointer press to
//! stdout. No text injection or gesture decoding yet (see ARCHITECTURE.md §5).

mod font;
mod input;
mod keyboard;
mod render;

use keyboard::{KeyAction, LayerKind, VisualKeyboard};
use render::{Canvas, Color};

/// Which base layer the keyboard is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Base {
    Letters,
    Symbols1,
    Symbols2,
}

/// Shift latch state for the letters layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shift {
    Off,
    /// Capitalize the next character, then revert.
    OneShot,
    /// Caps lock.
    Lock,
}

/// What the current suggestion-bar entries mean — so tapping one does the right
/// thing (complete the tapped word vs. replace the committed gesture word).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SuggestKind {
    None,
    /// Predictions for the word currently being tap-typed.
    Completions,
    /// Alternates for a word just committed by a gesture.
    GestureAlternates,
}

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use swype_decoder::{Decoder, Dictionary, KeyboardLayout, Point, Trace};

use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_layer, delegate_output, delegate_pointer, delegate_registry,
    delegate_seat, delegate_shm, delegate_touch,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        pointer::{PointerEvent, PointerEventKind, PointerHandler, BTN_LEFT},
        touch::TouchHandler,
        Capability, SeatHandler, SeatState,
    },
    shell::{
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
        WaylandSurface,
    },
    shm::{slot::SlotPool, Shm, ShmHandler},
};
use wayland_client::{
    globals::registry_queue_init,
    protocol::{wl_output, wl_pointer, wl_seat, wl_shm, wl_surface, wl_touch},
    Connection, Dispatch, Proxy, QueueHandle,
};
use wayland_protocols_misc::zwp_input_method_v2::client::{
    zwp_input_method_manager_v2::ZwpInputMethodManagerV2,
    zwp_input_method_v2::{self, ZwpInputMethodV2},
};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::{
    zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1,
    zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1,
};

/// Requested total surface height in pixels (suggestion bar + keys). Width spans
/// the output.
const KBD_HEIGHT: u32 = 300;
/// Height of the suggestion bar strip at the top of the surface.
const SUGGEST_H: u32 = 48;
/// A press that travels less than this fraction of a key width is a tap, not a
/// gesture.
const TAP_FRACTION: f32 = 0.45;
/// Smoothing passes applied to the rendered gesture trail (purely cosmetic; the
/// decoder smooths separately via `DecoderParams::smooth_passes`).
const TRAIL_SMOOTH_PASSES: usize = 2;

// --- palette ---------------------------------------------------------------
const COL_BG: Color = Color::rgb(0x1a, 0x1c, 0x22);
const COL_KEY: Color = Color::rgb(0x33, 0x37, 0x42);
const COL_KEY_FN: Color = Color::rgb(0x26, 0x2a, 0x33);
const COL_KEY_PRESSED: Color = Color::rgb(0x4c, 0x8b, 0xf5);
const COL_LABEL: Color = Color::rgb(0xe6, 0xe8, 0xee);
const COL_TRAIL: Color = Color::rgb(0x6f, 0xa8, 0xff);
const COL_SUGGEST_BG: Color = Color::rgb(0x12, 0x14, 0x18);
const COL_SUGGEST_SEL: Color = Color::rgb(0x2a, 0x3a, 0x5a);

fn main() {
    let conn = Connection::connect_to_env()
        .expect("failed to connect to a Wayland display (is WAYLAND_DISPLAY set?)");
    let (globals, mut event_queue) =
        registry_queue_init(&conn).expect("failed to init Wayland registry");
    let qh = event_queue.handle();

    let compositor =
        CompositorState::bind(&globals, &qh).expect("wl_compositor is not available");
    let layer_shell =
        LayerShell::bind(&globals, &qh).expect("zwlr_layer_shell_v1 is not available");
    let shm = Shm::bind(&globals, &qh).expect("wl_shm is not available");

    // The keyboard surface: bottom layer-shell, spanning the screen width, never
    // taking keyboard focus (so the focused app keeps typing into its field).
    let surface = compositor.create_surface(&qh);
    let layer =
        layer_shell.create_layer_surface(&qh, surface, Layer::Top, Some("swype-kbd"), None);
    layer.set_anchor(Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
    layer.set_size(0, KBD_HEIGHT);
    layer.set_exclusive_zone(KBD_HEIGHT as i32);
    layer.set_keyboard_interactivity(KeyboardInteractivity::None);
    layer.commit();

    let pool = SlotPool::new((1920 * KBD_HEIGHT * 4) as usize, &shm)
        .expect("failed to create shm slot pool");

    // Text-injection setup (ARCHITECTURE.md §3.3). Both managers are seat-global
    // and independent of the layer surface. Either may be absent on a
    // non-wlroots compositor; degrade gracefully with a warning.
    let seat_state = SeatState::new(&globals, &qh);
    let seat = seat_state.seats().next();
    if seat.is_none() {
        eprintln!("warning: no wl_seat; pointer input and text injection disabled");
    }

    let im_mgr = globals
        .bind::<ZwpInputMethodManagerV2, _, _>(&qh, 1..=1, ())
        .map_err(|e| eprintln!("warning: input-method-v2 unavailable ({e}); text path disabled"))
        .ok();
    let vk_mgr = globals
        .bind::<ZwpVirtualKeyboardManagerV1, _, _>(&qh, 1..=1, ())
        .map_err(|e| eprintln!("warning: virtual-keyboard-v1 unavailable ({e}); key synth disabled"))
        .ok();

    let input_method = match (&im_mgr, &seat) {
        (Some(m), Some(s)) => Some(m.get_input_method(s, &qh, ())),
        _ => None,
    };
    let virtual_keyboard = match (&vk_mgr, &seat) {
        (Some(m), Some(s)) => Some(m.create_virtual_keyboard(s, &qh, ())),
        _ => None,
    };
    // Upload the US keymap to the virtual keyboard once; keep the memfd alive.
    let keymap_file = virtual_keyboard.as_ref().and_then(|vk| match upload_keymap(vk) {
        Ok(f) => Some(f),
        Err(e) => {
            eprintln!("warning: failed to upload keymap ({e}); key synth disabled");
            None
        }
    });

    // The gesture decoder. Load the external ~50k unigram list if present, else
    // fall back to the small embedded list.
    let (dict, source) = load_dictionary();
    let t0 = Instant::now();
    let mut decoder = Decoder::with_defaults(KeyboardLayout::qwerty(), dict);
    // Apply persisted per-user learning on top of the base frequencies.
    let learned_path = learned_path();
    let learned = load_learned(&learned_path);
    for (word, boost) in &learned {
        decoder.learn(word, *boost);
    }
    eprintln!(
        "swype-kbd: decoder ready — {} word templates from {} (built in {} ms); {} learned words",
        decoder.templates_len(),
        source,
        t0.elapsed().as_millis(),
        learned.len(),
    );

    let mut app = App {
        registry_state: RegistryState::new(&globals),
        seat_state,
        output_state: OutputState::new(&globals, &qh),
        shm,
        pool,
        layer,
        keyboard: VisualKeyboard::new(),
        width: 0,
        height: KBD_HEIGHT,
        configured: false,
        pointer: None,
        touch: None,
        active_touch: None,
        pressed: None,
        exit: false,
        base: Base::Letters,
        shift: Shift::Off,
        auto_space: false,
        pending_cap: false,
        last_shift_tap: None,
        decoder,
        learned,
        learned_path,
        gesture: Vec::new(),
        gesture_start: None,
        pressing: false,
        suggestions: Vec::new(),
        last_committed: None,
        left: String::new(),
        suggest_kind: SuggestKind::None,
        undo: None,
        prev_dyn: None,
        _im_mgr: im_mgr,
        input_method,
        im_active: false,
        im_pending_active: false,
        im_serial: 0,
        _vk_mgr: vk_mgr,
        virtual_keyboard,
        _keymap_file: keymap_file,
        key_time: 0,
    };

    eprintln!("swype-kbd: layer surface created; waiting for configure…");
    while !app.exit {
        event_queue
            .blocking_dispatch(&mut app)
            .expect("wayland dispatch failed");
    }
}

struct App {
    registry_state: RegistryState,
    seat_state: SeatState,
    output_state: OutputState,
    shm: Shm,
    pool: SlotPool,
    layer: LayerSurface,
    keyboard: VisualKeyboard,
    width: u32,
    height: u32,
    configured: bool,
    pointer: Option<wl_pointer::WlPointer>,
    touch: Option<wl_touch::WlTouch>,
    /// The touch-point id currently driving a press/gesture (single-touch).
    active_touch: Option<i32>,
    /// Index into the current layer's caps currently held down, for highlight.
    pressed: Option<usize>,
    exit: bool,

    // --- input layers ---
    base: Base,
    shift: Shift,
    /// True right after a word was committed with a trailing auto-space, so the
    /// next punctuation can tuck in against the word (smart spacing).
    auto_space: bool,
    /// A sentence just ended; capitalize the next letter typed on the letters
    /// layer (auto-capitalization).
    pending_cap: bool,
    /// Timestamp of the last Shift tap, for double-tap caps-lock detection.
    last_shift_tap: Option<Instant>,

    // --- gesture decoding (ARCHITECTURE.md §4) ---
    decoder: Decoder,
    /// Per-user learned frequency boosts (lowercase word -> cumulative boost),
    /// persisted to `learned_path`.
    learned: HashMap<String, f32>,
    learned_path: PathBuf,
    /// In-progress swipe: surface-pixel `(x, y, ms)` samples while pressing.
    gesture: Vec<(f32, f32, u32)>,
    gesture_start: Option<Instant>,
    pressing: bool,
    /// Current suggestion-bar candidates (slot 0 is the committed word).
    suggestions: Vec<String>,
    /// The word last committed by a gesture, for tap-to-replace.
    last_committed: Option<String>,
    /// A local mirror of the text just left of the cursor (our own edits),
    /// bounded in length. The current word for prediction is derived as its
    /// trailing run of letters, so prediction survives backspacing over a space.
    left: String,
    /// Meaning of the current suggestion-bar entries.
    suggest_kind: SuggestKind,
    /// If set, the number of trailing characters (a just-committed word plus its
    /// space) that the very next Backspace removes as a unit — one-tap undo of a
    /// gesture/completion/replacement. Any other tapped key disarms it.
    undo: Option<u32>,
    /// Bounding box (x0, y0, x1, y1 in buffer pixels) of the dynamic overlay —
    /// trail, key pop-up, pressed-key highlight — drawn last frame. The partial
    /// damage path unions it with this frame's box so vacated pixels repaint.
    prev_dyn: Option<(i32, i32, i32, i32)>,

    // --- text injection (ARCHITECTURE.md §3.3) ---
    _im_mgr: Option<ZwpInputMethodManagerV2>,
    input_method: Option<ZwpInputMethodV2>,
    /// Whether a text field is currently focused (input-method `activate`d).
    im_active: bool,
    /// Pending activation state, applied on the next `done`.
    im_pending_active: bool,
    /// Count of `done` events received — the serial for `commit`.
    im_serial: u32,
    _vk_mgr: Option<ZwpVirtualKeyboardManagerV1>,
    virtual_keyboard: Option<ZwpVirtualKeyboardV1>,
    _keymap_file: Option<std::fs::File>,
    /// Monotonic timestamp for synthesized key events.
    key_time: u32,
}

/// Per-correction frequency boost. A correction rewards the chosen word (up to
/// `LEARN_CAP`) and decays the rejected word's accumulated boost back toward
/// zero — never negative, so a rejected word is never suppressed below its
/// natural rank and always stays pickable in the suggestions. The reward plus
/// the rejected word's decay together swing the score gap enough that one or two
/// corrections flip a confusable pair.
const LEARN_DELTA: f32 = 8.0;
const LEARN_CAP: f32 = 24.0;

/// Path to the persisted per-user learned-words file (XDG state dir).
fn learned_path() -> PathBuf {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("swype-kbd").join("learned.txt")
}

/// Load `word boost` lines into a map (lowercase keys). Missing file -> empty.
fn load_learned(path: &Path) -> HashMap<String, f32> {
    let mut map = HashMap::new();
    if let Ok(text) = std::fs::read_to_string(path) {
        for line in text.lines() {
            let mut it = line.split_whitespace();
            if let (Some(w), Some(b)) = (it.next(), it.next()) {
                if let Ok(boost) = b.parse::<f32>() {
                    map.insert(w.to_ascii_lowercase(), boost);
                }
            }
        }
    }
    map
}

/// Persist the learned-words map. Best-effort; errors are logged, not fatal.
fn save_learned(path: &Path, map: &HashMap<String, f32>) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let mut s = String::new();
    for (w, b) in map {
        s.push_str(&format!("{w} {b}\n"));
    }
    if let Err(e) = std::fs::write(path, s) {
        eprintln!("swype-kbd: could not save learned words to {}: {e}", path.display());
    }
}

/// Candidate paths for the external word+count dictionary, most specific first.
fn dict_paths() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Ok(p) = std::env::var("SWYPE_DICT") {
        v.push(PathBuf::from(p));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            v.push(dir.join("words_50k.txt"));
            // Repo layout: target/{debug,release}/swype-kbd -> ../../data/...
            v.push(dir.join("../../data/words_50k.txt"));
            // Installed layout: bin/swype-kbd -> ../share/swype-kbd/...
            v.push(dir.join("../share/swype-kbd/words_50k.txt"));
        }
    }
    v.push(PathBuf::from("data/words_50k.txt"));
    v
}

/// Load the dictionary from the first readable candidate path, falling back to
/// the embedded list. Returns the dictionary and a human-readable source label.
fn load_dictionary() -> (Dictionary, String) {
    for path in dict_paths() {
        if let Ok(text) = std::fs::read_to_string(&path) {
            let dict = Dictionary::parse_counts(&text);
            if !dict.is_empty() {
                return (dict, path.display().to_string());
            }
        }
    }
    eprintln!("swype-kbd: no external dictionary found (set SWYPE_DICT or place data/words_50k.txt); using embedded list");
    (Dictionary::english(), "embedded list".to_string())
}

/// Write the embedded keymap into a memfd and hand it to the virtual keyboard.
/// The returned `File` must be kept alive until the compositor has read the fd.
fn upload_keymap(vk: &ZwpVirtualKeyboardV1) -> Result<std::fs::File, Box<dyn std::error::Error>> {
    use std::io::Write;
    use std::os::fd::AsFd;

    let fd = rustix::fs::memfd_create("swype-keymap", rustix::fs::MemfdFlags::CLOEXEC)?;
    let mut file = std::fs::File::from(fd);
    file.write_all(input::KEYMAP.as_bytes())?;
    file.write_all(&[0])?; // wl_keyboard keymaps are NUL-terminated
    file.flush()?;
    let size = input::KEYMAP.len() as u32 + 1;
    vk.keymap(input::KEYMAP_FORMAT_XKB_V1, file.as_fd(), size);
    vk.modifiers(0, 0, 0, 0); // start from a known, all-released modifier state
    Ok(file)
}

impl App {
    /// Full repaint with whole-surface damage — for structural changes (layer or
    /// shift switch, suggestion-bar updates, commits, resize).
    fn draw(&mut self, qh: &QueueHandle<Self>) {
        self.render(qh, true);
    }

    /// Repaint reporting only the changed region — for the high-frequency motion
    /// path, where just the trail / pop-up / pressed key move. The whole buffer
    /// is still rendered (so it is always correct), but the compositor is told to
    /// re-composite only the dynamic band.
    fn draw_motion(&mut self, qh: &QueueHandle<Self>) {
        self.render(qh, false);
    }

    /// Repaint the whole keyboard into a fresh buffer and present it, damaging
    /// either the whole surface (`full`) or just the dynamic overlay region.
    fn render(&mut self, qh: &QueueHandle<Self>, full: bool) {
        if self.width == 0 || self.height == 0 {
            return;
        }
        // Bounding box of dynamic overlay drawn this frame (buffer-pixel corners).
        let mut dyn_box: Option<(i32, i32, i32, i32)> = None;
        let (w, h) = (self.width, self.height);
        let key_h = self.key_height();
        let off = SUGGEST_H as i32;
        let kind = self.current_kind();
        let shift_on = self.shift != Shift::Off;
        let stride = w as i32 * 4;
        let (buffer, canvas) = self
            .pool
            .create_buffer(w as i32, h as i32, stride, wl_shm::Format::Argb8888)
            .expect("failed to create buffer");
        {
            let mut cv = Canvas::new(canvas, w as usize, h as usize);
            cv.clear(COL_BG);

            // Suggestion bar (top strip).
            cv.fill_rect(0, 0, w as i32, SUGGEST_H as i32, COL_SUGGEST_BG);
            let n = self.suggestions.len().min(4);
            if n > 0 {
                let cellw = w as i32 / n as i32;
                let sscale = (SUGGEST_H / (font::GLYPH_H as u32 * 2)).clamp(2, 5) as usize;
                for (i, word) in self.suggestions.iter().take(4).enumerate() {
                    let cx = i as i32 * cellw;
                    if i == 0 {
                        cv.fill_rect(cx + 2, 2, cellw - 4, SUGGEST_H as i32 - 4, COL_SUGGEST_SEL);
                    }
                    cv.draw_text_centered(word, cx, 0, cellw, SUGGEST_H as i32, sscale, COL_LABEL);
                }
            }

            // Keys, drawn in the area below the suggestion bar.
            let scale = key_label_scale(key_h);
            for (i, cap) in self.keyboard.caps(kind).iter().enumerate() {
                let r = self.keyboard.rect_of(cap, w, key_h);
                // 2px gap between keys; offset by the suggestion bar height.
                let (kx, ky, kw, kh) = (r.x + 2, r.y + off + 2, r.w - 4, r.h - 4);
                let active_shift = cap.action == KeyAction::Shift && shift_on;
                let fill = if self.pressed == Some(i) || active_shift {
                    COL_KEY_PRESSED
                } else if matches!(cap.action, KeyAction::Char(_)) {
                    COL_KEY
                } else {
                    COL_KEY_FN
                };
                cv.fill_rect(kx, ky, kw, kh, fill);
                if !cap.label.is_empty() {
                    cv.draw_text_centered(&cap.label, kx, ky, kw, kh, scale, COL_LABEL);
                }
                // The pressed-key highlight is dynamic: it must repaint when the
                // press moves or lifts. (active_shift flips only on a full redraw.)
                if self.pressed == Some(i) {
                    union_box(&mut dyn_box, r.x, r.y + off, r.w, r.h);
                }
            }

            // Key pop-up preview: while a character key is held (a tap, not yet
            // a swipe), show a magnified copy above it — the finger covers the
            // real key on a touchscreen.
            if self.pressing {
                if let Some(i) = self.pressed {
                    let caps = self.keyboard.caps(kind);
                    if let Some(cap) = caps.get(i) {
                        if matches!(cap.action, KeyAction::Char(_)) && !cap.label.is_empty() {
                            let r = self.keyboard.rect_of(cap, w, key_h);
                            let pw = (r.w as f32 * 1.5) as i32;
                            let ph = (r.h as f32 * 1.3) as i32;
                            let px = (r.x + (r.w - pw) / 2).clamp(0, w as i32 - pw);
                            let py = (r.y + off - ph - 6).max(0);
                            cv.fill_rect(px, py, pw, ph, COL_KEY_PRESSED);
                            let pscale = (scale + 2).min(8);
                            cv.draw_text_centered(&cap.label, px, py, pw, ph, pscale, COL_LABEL);
                            union_box(&mut dyn_box, px, py, pw, ph);
                        }
                    }
                }
            }

            // Live gesture trail, smoothed so finger jitter doesn't render as a
            // ragged line. Endpoints are held fixed, so the trail still tracks
            // the fingertip exactly. Cheap: gestures are at most a few hundred
            // points and this runs only while pressing.
            if self.pressing && self.gesture.len() >= 2 {
                let raw = Trace::from_points(
                    self.gesture.iter().map(|&(x, y, t)| Point::new(x, y, t)).collect(),
                );
                let trail = raw.smoothed(TRAIL_SMOOTH_PASSES);
                for win in trail.points.windows(2) {
                    cv.draw_line(
                        win[0].x as i32,
                        win[0].y as i32,
                        win[1].x as i32,
                        win[1].y as i32,
                        4,
                        COL_TRAIL,
                    );
                }
                // Trail bbox, inflated past the 4px line width + smoothing slack.
                for p in &trail.points {
                    union_box(&mut dyn_box, p.x as i32 - 4, p.y as i32 - 4, 8, 8);
                }
            }
        }

        // Damage: the whole surface for a full repaint, else the union of this
        // frame's dynamic box and last frame's (so pixels the overlay vacated are
        // recomposited). The buffer itself is always fully rendered, so partial
        // damage can never leave a stale region — it only spares the compositor.
        let (dx, dy, dw, dh) = if full {
            (0, 0, w as i32, h as i32)
        } else {
            let mut u = self.prev_dyn;
            if let Some((x0, y0, x1, y1)) = dyn_box {
                union_box(&mut u, x0, y0, x1 - x0, y1 - y0);
            }
            match u {
                Some((x0, y0, x1, y1)) => {
                    let x0 = x0.clamp(0, w as i32);
                    let y0 = y0.clamp(0, h as i32);
                    let x1 = x1.clamp(0, w as i32);
                    let y1 = y1.clamp(0, h as i32);
                    (x0, y0, x1 - x0, y1 - y0)
                }
                None => (0, 0, 0, 0),
            }
        };
        self.prev_dyn = dyn_box;
        if dw > 0 && dh > 0 {
            self.layer.wl_surface().damage_buffer(dx, dy, dw, dh);
        }
        buffer
            .attach_to(self.layer.wl_surface())
            .expect("failed to attach buffer");
        self.layer.commit();
        let _ = qh;
    }

    /// Usable key-area height (total minus the suggestion bar).
    fn key_height(&self) -> u32 {
        self.height.saturating_sub(SUGGEST_H)
    }

    /// The layer currently displayed, derived from base layer + shift state.
    fn current_kind(&self) -> LayerKind {
        match self.base {
            Base::Letters if self.shift != Shift::Off => LayerKind::LettersUpper,
            Base::Letters => LayerKind::LettersLower,
            Base::Symbols1 => LayerKind::Symbols1,
            Base::Symbols2 => LayerKind::Symbols2,
        }
    }

    /// Map a surface pixel to decoder keyboard-units. The key area is 10 units
    /// wide and 4 rows tall, so letter-row centers land at y = 0.5/1.5/2.5,
    /// matching the decoder's QWERTY centroids.
    fn px_to_unit(&self, x: f32, y: f32, t: u32) -> Point {
        let ux = (self.width as f32 / 10.0).max(1.0);
        let uy = (self.key_height() as f32 / 4.0).max(1.0);
        Point::new(x / ux, (y - SUGGEST_H as f32) / uy, t)
    }

    fn on_press(&mut self, x: f64, y: f64, qh: &QueueHandle<Self>) {
        let (px, py) = (x as f32, y as f32);

        // A press in the suggestion bar acts on the tapped word: complete the
        // word being typed, or replace the word a gesture just committed.
        if (py as u32) < SUGGEST_H {
            if let Some(word) = self.suggestion_at(px) {
                match self.suggest_kind {
                    SuggestKind::Completions => self.complete_with(&word),
                    SuggestKind::GestureAlternates => self.replace_with(&word),
                    SuggestKind::None => {}
                }
                self.draw(qh);
            }
            return;
        }

        // Otherwise begin capturing a press in the key area. Whether it is a tap
        // or a gesture is decided on release.
        self.gesture.clear();
        self.gesture_start = Some(Instant::now());
        self.gesture.push((px, py, 0));
        self.pressing = true;
        self.pressed = self.keyboard.hit_index(
            self.current_kind(),
            px as i32,
            py as i32 - SUGGEST_H as i32,
            self.width,
            self.key_height(),
        );
        self.draw(qh);
    }

    fn on_motion(&mut self, x: f64, y: f64, qh: &QueueHandle<Self>) {
        if !self.pressing {
            return;
        }
        let t = self
            .gesture_start
            .map(|s| s.elapsed().as_millis() as u32)
            .unwrap_or(0);
        self.gesture.push((x as f32, y as f32, t));
        // Once the press has clearly become a swipe, drop the key highlight.
        if self.pressed.is_some() && pixel_path_len(&self.gesture) > self.tap_threshold() {
            self.pressed = None;
        }
        self.draw_motion(qh);
    }

    fn on_release(&mut self, qh: &QueueHandle<Self>) {
        if !self.pressing {
            return;
        }
        self.pressing = false;

        // Gestures only make sense on the letters layer; elsewhere every press
        // is a tap.
        let is_gesture = self.base == Base::Letters
            && self.gesture.len() >= 3
            && pixel_path_len(&self.gesture) > self.tap_threshold();
        if is_gesture {
            self.run_gesture();
        } else if let Some(i) = self.pressed {
            let action = self.keyboard.caps(self.current_kind())[i].action;
            self.handle_action(action);
        }
        self.pressed = None;
        self.gesture.clear();
        self.draw(qh);
    }

    /// Abandon an in-progress press without committing anything (touch cancel).
    fn on_cancel(&mut self, qh: &QueueHandle<Self>) {
        self.pressing = false;
        self.pressed = None;
        self.gesture.clear();
        self.draw(qh);
    }

    /// Minimum pixel travel for a press to count as a gesture.
    fn tap_threshold(&self) -> f32 {
        (self.width as f32 / 10.0) * TAP_FRACTION
    }

    /// Decode the captured swipe and commit the top candidate, populating the
    /// suggestion bar with the alternates.
    fn run_gesture(&mut self) {
        let pts: Vec<Point> = self
            .gesture
            .iter()
            .map(|&(x, y, t)| self.px_to_unit(x, y, t))
            .collect();
        let trace = Trace::from_points(pts);
        let t0 = Instant::now();
        let cands = self.decoder.decode(&trace);
        let decode_us = t0.elapsed().as_micros();
        match cands.first() {
            Some(top) => {
                // Apply shift to the committed word (sentence-case).
                let word = if self.shift != Shift::Off {
                    capitalize(&top.word)
                } else {
                    top.word.clone()
                };
                if self.shift == Shift::OneShot {
                    self.shift = Shift::Off;
                }
                self.pending_cap = false; // a word isn't a sentence end
                self.commit_word(&word);
                self.suggestions = cands
                    .iter()
                    .take(4)
                    .map(|c| {
                        if word.starts_with(|ch: char| ch.is_uppercase()) {
                            capitalize(&c.word)
                        } else {
                            c.word.clone()
                        }
                    })
                    .collect();
                self.suggest_kind = SuggestKind::GestureAlternates;
                // A backspace now removes the whole swiped word in one tap.
                self.undo = Some(word.len() as u32 + 1);
                let alts: Vec<&String> = self.suggestions.iter().skip(1).collect();
                println!("gesture -> '{word}'  alternates: {alts:?}  ({decode_us} us)");
            }
            None => println!("gesture -> (no candidates)  ({decode_us} us)"),
        }
    }

    /// Commit a decoded word followed by a space, via whichever channel is live.
    fn commit_word(&mut self, word: &str) {
        let text = format!("{word} ");
        if !self.commit_text(&text) {
            for ch in text.chars() {
                self.vk_type_char(ch);
            }
        }
        for ch in text.chars() {
            self.push_left(ch);
        }
        self.last_committed = Some(word.to_string());
        self.auto_space = true;
    }

    /// Replace the previously committed gesture word with `word` (tap-to-replace
    /// from the suggestion bar): delete the old word+space, commit the new one.
    fn replace_with(&mut self, word: &str) {
        // The word being replaced is the one the user is rejecting.
        let rejected = self.last_committed.clone();
        if let Some(prev) = &rejected {
            self.delete_n(prev.len() as u32 + 1); // +1 for the trailing space (ASCII)
        }
        self.commit_word(word);
        // A backspace now undoes this replacement (removes the new word).
        self.undo = Some(word.len() as u32 + 1);
        // Move the chosen word into slot 0 so it shows as selected.
        if let Some(pos) = self.suggestions.iter().position(|w| w == word) {
            self.suggestions.swap(0, pos);
        }
        // Learn relatively: reward the chosen word, dock the rejected one, so a
        // confusable pair converges instead of the first-boosted word sticking.
        self.adjust_learning(word, LEARN_DELTA);
        if let Some(r) = &rejected {
            if r.to_ascii_lowercase() != word.to_ascii_lowercase() {
                self.adjust_learning(r, -LEARN_DELTA);
            }
        }
        save_learned(&self.learned_path, &self.learned);
        println!("replace -> '{word}' (rejected {rejected:?})");
    }

    /// Nudge a word's learned frequency boost by `delta`, clamped to
    /// `[0, LEARN_CAP]` (never negative, so a penalized word only loses its
    /// accumulated boost, never drops below base rank), and apply the change to
    /// the live decoder.
    fn adjust_learning(&mut self, word: &str, delta: f32) {
        let key = word.to_ascii_lowercase();
        let old = self.learned.get(&key).copied().unwrap_or(0.0);
        let new = (old + delta).clamp(0.0, LEARN_CAP);
        let applied = new - old;
        if applied.abs() < f32::EPSILON {
            return; // already at the clamp
        }
        if new.abs() < f32::EPSILON {
            self.learned.remove(&key);
        } else {
            self.learned.insert(key.clone(), new);
        }
        self.decoder.learn(&key, applied);
        println!("learned '{key}' -> {new:+.1}");
    }

    /// Which suggestion (if any) sits under pixel x in the suggestion bar.
    fn suggestion_at(&self, px: f32) -> Option<String> {
        let n = self.suggestions.len().min(4);
        if n == 0 {
            return None;
        }
        let cell = self.width as f32 / n as f32;
        let idx = ((px / cell) as usize).min(n - 1);
        Some(self.suggestions[idx].clone())
    }

    /// Act on a tapped key.
    ///
    /// Text (letters, space, punctuation, symbols) goes through input-method
    /// `commit_string` when a field is active, else falls back to synthesized
    /// keystrokes (with Shift for capitals/shifted symbols). Backspace and Enter
    /// are always real key events. The remaining actions switch layers or latch
    /// shift; a one-shot shift is consumed after the next character.
    fn handle_action(&mut self, action: KeyAction) {
        // Any tapped key consumes the one-tap undo arming; only Backspace acts on
        // it (below). After this, a second backspace deletes normally.
        let undo = self.undo.take();
        match action {
            KeyAction::Char(c) => self.type_key_char(c),
            KeyAction::Space => {
                if !self.commit_text(" ") {
                    self.vk_type_char(' ');
                }
                self.push_left(' ');
                self.auto_space = false;
            }
            KeyAction::Backspace => {
                if let Some(n) = undo {
                    // Undo the just-committed word: remove it and its trailing
                    // space as a unit instead of one letter at a time, and drop
                    // the now-stale suggestion bar.
                    self.delete_n(n);
                    self.suggestions.clear();
                    self.suggest_kind = SuggestKind::None;
                    self.last_committed = None;
                    println!("undo committed word ({n} chars)");
                } else {
                    self.vk_tap(input::KEY_BACKSPACE);
                    self.left.pop();
                }
                self.auto_space = false;
                self.pending_cap = false;
            }
            KeyAction::Enter => {
                self.vk_tap(input::KEY_ENTER);
                self.push_left('\n');
                self.auto_space = false;
                self.pending_cap = true; // new line starts a sentence
            }
            KeyAction::Shift => {
                // Single tap toggles shift on/off; a quick double-tap latches
                // caps lock (matching iOS/Gboard). From any on-state a single
                // tap returns to Off.
                let now = Instant::now();
                let double_tap = self
                    .last_shift_tap
                    .map(|t| now.duration_since(t) < Duration::from_millis(400))
                    .unwrap_or(false);
                self.last_shift_tap = Some(now);
                self.shift = if double_tap {
                    Shift::Lock
                } else if self.shift == Shift::Off {
                    Shift::OneShot
                } else {
                    Shift::Off
                };
                self.pending_cap = false; // explicit shift overrides auto-cap
            }
            KeyAction::ToLetters => self.base = Base::Letters,
            KeyAction::ToSymbols1 => {
                self.base = Base::Symbols1;
                self.shift = Shift::Off;
            }
            KeyAction::ToSymbols2 => self.base = Base::Symbols2,
        }
        self.apply_autocap();
        self.refresh_completions();
        print_action(action); // keep the stdout trace for debugging
    }

    /// Type a tapped character, applying smart spacing (tuck punctuation against
    /// a preceding auto-spaced word) and arming auto-capitalization after a
    /// sentence end.
    fn type_key_char(&mut self, c: char) {
        if self.auto_space && is_tight_punct(c) {
            // "word ." -> "word. ": drop the trailing space, then "<punct> ".
            self.delete_one();
            self.type_char(c);
            self.type_char(' ');
            self.auto_space = true;
        } else {
            self.type_char(c);
            self.auto_space = false;
        }
        if c.is_ascii_alphabetic() && self.shift == Shift::OneShot {
            self.shift = Shift::Off;
        }
        self.pending_cap = is_sentence_end(c);
    }

    /// The word for prediction: the trailing run of letters left of the cursor.
    fn current_word(&self) -> String {
        let rev: String = self
            .left
            .chars()
            .rev()
            .take_while(|c| c.is_ascii_alphabetic())
            .collect();
        rev.chars().rev().collect()
    }

    /// Recompute predictive completions from the word left of the cursor.
    fn refresh_completions(&mut self) {
        let word = if self.base == Base::Letters {
            self.current_word()
        } else {
            String::new()
        };
        if word.is_empty() {
            // Don't disturb gesture alternates; just drop stale completions.
            if self.suggest_kind == SuggestKind::Completions {
                self.suggestions.clear();
                self.suggest_kind = SuggestKind::None;
            }
            return;
        }
        let comps = self.decoder.complete(&word, 4);
        if comps.is_empty() {
            if self.suggest_kind == SuggestKind::Completions {
                self.suggestions.clear();
                self.suggest_kind = SuggestKind::None;
            }
            return;
        }
        let cap = word.chars().next().is_some_and(|c| c.is_uppercase());
        self.suggestions = comps
            .iter()
            .map(|w| if cap { capitalize(w) } else { w.clone() })
            .collect();
        self.suggest_kind = SuggestKind::Completions;
    }

    /// Finish the current word from a tapped prediction: type the part after the
    /// already-typed prefix plus a space.
    fn complete_with(&mut self, word: &str) {
        let pre = self.current_word().chars().count();
        let suffix: String = word.chars().skip(pre).collect();
        for ch in suffix.chars() {
            self.type_char(ch);
        }
        self.type_char(' ');
        self.auto_space = true;
        self.last_committed = Some(word.to_ascii_lowercase());
        // A backspace now undoes the completion, restoring the typed prefix.
        self.undo = Some(suffix.len() as u32 + 1);
        self.refresh_completions();
        println!("complete -> '{word}'");
    }

    /// If a sentence just ended, latch a one-shot shift so the next letter
    /// capitalizes (only when the user hasn't set shift themselves).
    fn apply_autocap(&mut self) {
        if self.pending_cap && self.base == Base::Letters && self.shift == Shift::Off {
            self.shift = Shift::OneShot;
        }
    }

    /// Type one character: input-method commit when a field is active, else a
    /// synthesized keystroke. Mirrors the character into `left`.
    fn type_char(&mut self, c: char) {
        let mut buf = [0u8; 4];
        if !self.commit_text(c.encode_utf8(&mut buf)) {
            self.vk_type_char(c);
        }
        self.push_left(c);
    }

    /// Delete the `n` characters before the cursor via whichever channel is live,
    /// keeping the `left` mirror in sync. Dictionary words and the auto space are
    /// ASCII, so the char count equals the input-method byte length.
    fn delete_n(&mut self, n: u32) {
        if n == 0 {
            return;
        }
        if self.im_active {
            if let Some(im) = &self.input_method {
                im.delete_surrounding_text(n, 0);
                im.commit(self.im_serial);
            }
        } else {
            for _ in 0..n {
                self.vk_tap(input::KEY_BACKSPACE);
            }
        }
        for _ in 0..n {
            self.left.pop();
        }
    }

    /// Delete one character before the cursor via whichever channel is live, and
    /// mirror the deletion into `left`.
    fn delete_one(&mut self) {
        if self.im_active {
            if let Some(im) = &self.input_method {
                im.delete_surrounding_text(1, 0);
                im.commit(self.im_serial);
                self.left.pop();
                return;
            }
        }
        self.vk_tap(input::KEY_BACKSPACE);
        self.left.pop();
    }

    /// Append a character to the bounded left-of-cursor mirror.
    fn push_left(&mut self, c: char) {
        self.left.push(c);
        // Bound the mirror; only the trailing word matters for prediction.
        const MAX_LEFT: usize = 256;
        if self.left.len() > MAX_LEFT {
            let start = self.left.len() - MAX_LEFT;
            // Snap to a char boundary.
            let start = (start..self.left.len())
                .find(|&i| self.left.is_char_boundary(i))
                .unwrap_or(self.left.len());
            self.left.drain(..start);
        }
    }

    /// Commit text via the input-method channel. Returns false if no field is
    /// active (caller should fall back to the virtual keyboard).
    fn commit_text(&self, text: &str) -> bool {
        match (self.im_active, &self.input_method) {
            (true, Some(im)) => {
                im.commit_string(text.to_string());
                im.commit(self.im_serial);
                true
            }
            _ => false,
        }
    }

    /// Synthesize a press+release of an evdev keycode via the virtual keyboard.
    fn vk_tap(&mut self, keycode: u32) {
        let Some(vk) = self.virtual_keyboard.clone() else {
            return;
        };
        let t = self.next_time();
        vk.key(t, keycode, input::STATE_PRESSED);
        let t = self.next_time();
        vk.key(t, keycode, input::STATE_RELEASED);
    }

    /// Type one character via the virtual keyboard, holding Shift if the US
    /// layout requires it (capitals, shifted symbols). Used when no text field
    /// is active.
    fn vk_type_char(&mut self, c: char) {
        let Some((code, shift)) = input::char_to_key(c) else {
            return;
        };
        let Some(vk) = self.virtual_keyboard.clone() else {
            return;
        };
        // xkb Shift modifier mask is bit 0.
        if shift {
            vk.modifiers(1, 0, 0, 0);
        }
        let t = self.next_time();
        vk.key(t, code, input::STATE_PRESSED);
        let t = self.next_time();
        vk.key(t, code, input::STATE_RELEASED);
        if shift {
            vk.modifiers(0, 0, 0, 0);
        }
    }

    fn next_time(&mut self) -> u32 {
        let t = self.key_time;
        self.key_time = self.key_time.wrapping_add(1);
        t
    }
}

/// Total length of a pixel polyline.
fn pixel_path_len(g: &[(f32, f32, u32)]) -> f32 {
    g.windows(2)
        .map(|w| {
            let dx = w[1].0 - w[0].0;
            let dy = w[1].1 - w[0].1;
            (dx * dx + dy * dy).sqrt()
        })
        .sum()
}

/// Grow an `(x0, y0, x1, y1)` corner box to include the rect `(x, y, w, h)`.
fn union_box(acc: &mut Option<(i32, i32, i32, i32)>, x: i32, y: i32, w: i32, h: i32) {
    let (x0, y0, x1, y1) = (x, y, x + w, y + h);
    *acc = Some(match *acc {
        None => (x0, y0, x1, y1),
        Some((ax0, ay0, ax1, ay1)) => (ax0.min(x0), ay0.min(y0), ax1.max(x1), ay1.max(y1)),
    });
}

/// Scale factor for key labels, derived from row height so labels stay legible.
fn key_label_scale(height: u32) -> usize {
    // 4 rows; aim for a glyph ~1/3 of the row height.
    let row_h = height / 4;
    ((row_h / (font::GLYPH_H as u32 * 2)).max(1)).min(6) as usize
}

/// Punctuation that should sit tight against the preceding word (no space
/// before it) under smart spacing.
fn is_tight_punct(c: char) -> bool {
    matches!(c, '.' | ',' | '!' | '?' | ';' | ':')
}

/// Punctuation that ends a sentence (arms auto-capitalization).
fn is_sentence_end(c: char) -> bool {
    matches!(c, '.' | '!' | '?')
}

/// Uppercase the first character of a word.
fn capitalize(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

fn print_action(action: KeyAction) {
    match action {
        KeyAction::Char(c) => println!("key: '{c}'"),
        KeyAction::Space => println!("key: SPACE"),
        KeyAction::Backspace => println!("key: BACKSPACE"),
        KeyAction::Enter => println!("key: ENTER"),
        KeyAction::Shift => println!("key: SHIFT"),
        KeyAction::ToLetters => println!("key: ABC"),
        KeyAction::ToSymbols1 => println!("key: SYM1"),
        KeyAction::ToSymbols2 => println!("key: SYM2"),
    }
}

// --- Wayland handlers ------------------------------------------------------

impl CompositorHandler for App {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: i32,
    ) {
    }

    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }

    fn frame(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: u32,
    ) {
    }

    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}

impl LayerShellHandler for App {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &LayerSurface) {
        self.exit = true;
    }

    fn configure(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        _: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        let (mut w, mut h) = configure.new_size;
        if w == 0 {
            w = 1920;
        }
        if h == 0 {
            h = KBD_HEIGHT;
        }
        self.width = w;
        self.height = h;
        if !self.configured {
            self.configured = true;
            eprintln!("swype-kbd: configured {w}x{h}; rendering keyboard.");
        }
        self.draw(qh);
    }
}

impl SeatHandler for App {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}

    fn new_capability(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer && self.pointer.is_none() {
            self.pointer = Some(
                self.seat_state
                    .get_pointer(qh, &seat)
                    .expect("failed to get pointer"),
            );
        }
        if capability == Capability::Touch && self.touch.is_none() {
            match self.seat_state.get_touch(qh, &seat) {
                Ok(t) => self.touch = Some(t),
                Err(e) => eprintln!("swype-kbd: failed to get touch: {e}"),
            }
        }
    }

    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer {
            if let Some(p) = self.pointer.take() {
                p.release();
            }
        }
        if capability == Capability::Touch {
            if let Some(t) = self.touch.take() {
                t.release();
            }
            self.active_touch = None;
        }
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl PointerHandler for App {
    fn pointer_frame(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        _: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        for event in events {
            // Only react to events on our own surface.
            if event.surface != *self.layer.wl_surface() {
                continue;
            }
            match event.kind {
                PointerEventKind::Press { button, .. } if button == BTN_LEFT => {
                    let (x, y) = event.position;
                    self.on_press(x, y, qh);
                }
                PointerEventKind::Motion { .. } => {
                    let (x, y) = event.position;
                    self.on_motion(x, y, qh);
                }
                PointerEventKind::Release { button, .. } if button == BTN_LEFT => {
                    self.on_release(qh);
                }
                // Pointer left the surface mid-swipe: finalize what we have.
                PointerEventKind::Leave { .. } => {
                    self.on_release(qh);
                }
                _ => {}
            }
        }
    }
}

impl TouchHandler for App {
    fn down(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        _: &wl_touch::WlTouch,
        _serial: u32,
        _time: u32,
        surface: wl_surface::WlSurface,
        id: i32,
        position: (f64, f64),
    ) {
        // Single-touch: the first finger down drives the press/gesture; ignore
        // additional simultaneous touches until it lifts.
        if surface != *self.layer.wl_surface() || self.active_touch.is_some() {
            return;
        }
        self.active_touch = Some(id);
        self.on_press(position.0, position.1, qh);
    }

    fn up(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        _: &wl_touch::WlTouch,
        _serial: u32,
        _time: u32,
        id: i32,
    ) {
        if self.active_touch == Some(id) {
            self.active_touch = None;
            self.on_release(qh);
        }
    }

    fn motion(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        _: &wl_touch::WlTouch,
        _time: u32,
        id: i32,
        position: (f64, f64),
    ) {
        if self.active_touch == Some(id) {
            self.on_motion(position.0, position.1, qh);
        }
    }

    fn cancel(&mut self, _: &Connection, qh: &QueueHandle<Self>, _: &wl_touch::WlTouch) {
        // The compositor took over the touch sequence (e.g. a system gesture):
        // abandon the in-progress press without committing.
        self.active_touch = None;
        self.on_cancel(qh);
    }

    fn shape(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_touch::WlTouch,
        _id: i32,
        _major: f64,
        _minor: f64,
    ) {
    }

    fn orientation(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_touch::WlTouch,
        _id: i32,
        _orientation: f64,
    ) {
    }
}

impl ShmHandler for App {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState, SeatState];
}

delegate_compositor!(App);
delegate_output!(App);
delegate_shm!(App);
delegate_seat!(App);
delegate_pointer!(App);
delegate_touch!(App);
delegate_layer!(App);
delegate_registry!(App);

// --- input-method-v2 / virtual-keyboard-v1 (not covered by SCTK delegates) ---

impl Dispatch<ZwpInputMethodV2, ()> for App {
    fn event(
        state: &mut Self,
        _: &ZwpInputMethodV2,
        event: zwp_input_method_v2::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use zwp_input_method_v2::Event;
        match event {
            // activate/deactivate are pending until the batch's `done`.
            Event::Activate => state.im_pending_active = true,
            Event::Deactivate => state.im_pending_active = false,
            Event::Done => {
                state.im_active = state.im_pending_active;
                state.im_serial = state.im_serial.wrapping_add(1);
            }
            Event::Unavailable => {
                eprintln!("input-method: another input method is active; using key synth only");
                state.input_method = None;
                state.im_active = false;
            }
            // surrounding_text / text_change_cause / content_type: unused in M2.
            _ => {}
        }
    }
}

// The two managers and the virtual keyboard emit no events.
impl Dispatch<ZwpInputMethodManagerV2, ()> for App {
    fn event(
        _: &mut Self,
        _: &ZwpInputMethodManagerV2,
        _: <ZwpInputMethodManagerV2 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwpVirtualKeyboardManagerV1, ()> for App {
    fn event(
        _: &mut Self,
        _: &ZwpVirtualKeyboardManagerV1,
        _: <ZwpVirtualKeyboardManagerV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwpVirtualKeyboardV1, ()> for App {
    fn event(
        _: &mut Self,
        _: &ZwpVirtualKeyboardV1,
        _: <ZwpVirtualKeyboardV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
