//! swype-kbd — milestone 1.
//!
//! A `zwlr_layer_shell_v1` client that docks a software-rendered QWERTY keyboard
//! to the bottom of the screen and prints the key hit by each pointer press to
//! stdout. No text injection or gesture decoding yet (see ARCHITECTURE.md §5).

mod font;
mod input;
mod keyboard;
mod render;

use keyboard::{KeyAction, VisualKeyboard};
use render::{Canvas, Color};

use std::path::PathBuf;
use std::time::Instant;
use swype_decoder::{Decoder, Dictionary, KeyboardLayout, Point, Trace};

use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_layer, delegate_output, delegate_pointer, delegate_registry,
    delegate_seat, delegate_shm,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        pointer::{PointerEvent, PointerEventKind, PointerHandler, BTN_LEFT},
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
    protocol::{wl_output, wl_pointer, wl_seat, wl_shm, wl_surface},
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
    let decoder = Decoder::with_defaults(KeyboardLayout::qwerty(), dict);
    eprintln!(
        "swype-kbd: decoder ready — {} word templates from {} (built in {} ms)",
        decoder.templates_len(),
        source,
        t0.elapsed().as_millis()
    );

    let mut app = App {
        registry_state: RegistryState::new(&globals),
        seat_state,
        output_state: OutputState::new(&globals, &qh),
        shm,
        pool,
        layer,
        keyboard: VisualKeyboard::qwerty(),
        width: 0,
        height: KBD_HEIGHT,
        configured: false,
        pointer: None,
        pressed: None,
        exit: false,
        decoder,
        gesture: Vec::new(),
        gesture_start: None,
        pressing: false,
        suggestions: Vec::new(),
        last_committed: None,
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
    /// Index into `keyboard.caps()` currently held down, for press highlight.
    pressed: Option<usize>,
    exit: bool,

    // --- gesture decoding (ARCHITECTURE.md §4) ---
    decoder: Decoder,
    /// In-progress swipe: surface-pixel `(x, y, ms)` samples while pressing.
    gesture: Vec<(f32, f32, u32)>,
    gesture_start: Option<Instant>,
    pressing: bool,
    /// Current suggestion-bar candidates (slot 0 is the committed word).
    suggestions: Vec<String>,
    /// The word last committed by a gesture, for tap-to-replace.
    last_committed: Option<String>,

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
    /// Repaint the whole keyboard into a fresh buffer and present it.
    fn draw(&mut self, qh: &QueueHandle<Self>) {
        if self.width == 0 || self.height == 0 {
            return;
        }
        let (w, h) = (self.width, self.height);
        let key_h = self.key_height();
        let off = SUGGEST_H as i32;
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
            for (i, cap) in self.keyboard.caps().iter().enumerate() {
                let r = self.keyboard.rect_of(cap, w, key_h);
                // 2px gap between keys; offset by the suggestion bar height.
                let (kx, ky, kw, kh) = (r.x + 2, r.y + off + 2, r.w - 4, r.h - 4);
                let pressed = self.pressed == Some(i);
                let fill = if pressed {
                    COL_KEY_PRESSED
                } else if matches!(cap.action, KeyAction::Char(c) if c.is_ascii_alphabetic()) {
                    COL_KEY
                } else {
                    COL_KEY_FN
                };
                cv.fill_rect(kx, ky, kw, kh, fill);
                if !cap.label.is_empty() {
                    cv.draw_text_centered(&cap.label, kx, ky, kw, kh, scale, COL_LABEL);
                }
            }

            // Live gesture trail.
            if self.pressing && self.gesture.len() >= 2 {
                for win in self.gesture.windows(2) {
                    cv.draw_line(
                        win[0].0 as i32,
                        win[0].1 as i32,
                        win[1].0 as i32,
                        win[1].1 as i32,
                        4,
                        COL_TRAIL,
                    );
                }
            }
        }

        self.layer
            .wl_surface()
            .damage_buffer(0, 0, w as i32, h as i32);
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

        // A press in the suggestion bar replaces the committed word.
        if (py as u32) < SUGGEST_H {
            if let Some(word) = self.suggestion_at(px) {
                self.replace_with(&word);
                self.draw(qh);
            }
            return;
        }

        // Otherwise begin capturing a press in the key area. Whether it is a tap
        // or a gesture is decided on release. Starting fresh clears any prior
        // suggestions / committed-word state.
        self.suggestions.clear();
        self.last_committed = None;
        self.gesture.clear();
        self.gesture_start = Some(Instant::now());
        self.gesture.push((px, py, 0));
        self.pressing = true;
        self.pressed =
            self.keyboard
                .hit_index(px as i32, py as i32 - SUGGEST_H as i32, self.width, self.key_height());
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
        self.draw(qh);
    }

    fn on_release(&mut self, qh: &QueueHandle<Self>) {
        if !self.pressing {
            return;
        }
        self.pressing = false;

        let is_gesture = self.gesture.len() >= 3 && pixel_path_len(&self.gesture) > self.tap_threshold();
        if is_gesture {
            self.run_gesture();
        } else if let Some(i) = self.pressed {
            let action = self.keyboard.caps()[i].action;
            self.handle_action(action);
        }
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
                let word = top.word.clone();
                self.commit_word(&word);
                self.suggestions = cands.iter().take(4).map(|c| c.word.clone()).collect();
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
                if let Some(code) = input::char_to_keycode(ch) {
                    self.vk_tap(code);
                }
            }
        }
        self.last_committed = Some(word.to_string());
    }

    /// Replace the previously committed gesture word with `word` (tap-to-replace
    /// from the suggestion bar): delete the old word+space, commit the new one.
    fn replace_with(&mut self, word: &str) {
        if let Some(prev) = self.last_committed.clone() {
            let del = (prev.len() + 1) as u32; // +1 for the trailing space (ASCII)
            if self.im_active {
                if let Some(im) = &self.input_method {
                    im.delete_surrounding_text(del, 0);
                    im.commit(self.im_serial);
                }
            } else {
                for _ in 0..del {
                    self.vk_tap(input::KEY_BACKSPACE);
                }
            }
        }
        self.commit_word(word);
        // Move the chosen word into slot 0 so it shows as selected.
        if let Some(pos) = self.suggestions.iter().position(|w| w == word) {
            self.suggestions.swap(0, pos);
        }
        println!("replace -> '{word}'");
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

    /// Inject the result of a key press into the focused application.
    ///
    /// Text (letters, space, punctuation) goes through input-method
    /// `commit_string` when a field is active, else falls back to synthesized
    /// keystrokes. Backspace and Enter are always real key events: apps treat
    /// them as keys, so they correctly delete a selection, repeat, submit forms,
    /// etc. (`delete_surrounding_text` is reserved for gesture word-replace in
    /// milestone 4, where we delete a known committed range, not a selection.)
    fn handle_action(&mut self, action: KeyAction) {
        match action {
            KeyAction::Char(c) => {
                let mut buf = [0u8; 4];
                if !self.commit_text(c.encode_utf8(&mut buf)) {
                    if let Some(code) = input::char_to_keycode(c) {
                        self.vk_tap(code);
                    }
                }
            }
            KeyAction::Space => {
                if !self.commit_text(" ") {
                    self.vk_tap(input::char_to_keycode(' ').unwrap());
                }
            }
            KeyAction::Backspace => self.vk_tap(input::KEY_BACKSPACE),
            KeyAction::Enter => self.vk_tap(input::KEY_ENTER),
            KeyAction::Shift | KeyAction::Symbols => {} // not wired in milestone 2
        }
        print_action(action); // keep the M1 stdout trace for debugging
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

/// Scale factor for key labels, derived from row height so labels stay legible.
fn key_label_scale(height: u32) -> usize {
    // 4 rows; aim for a glyph ~1/3 of the row height.
    let row_h = height / 4;
    ((row_h / (font::GLYPH_H as u32 * 2)).max(1)).min(6) as usize
}

fn print_action(action: KeyAction) {
    match action {
        KeyAction::Char(c) => println!("key: '{c}'"),
        KeyAction::Space => println!("key: SPACE"),
        KeyAction::Backspace => println!("key: BACKSPACE"),
        KeyAction::Enter => println!("key: ENTER"),
        KeyAction::Shift => println!("key: SHIFT"),
        KeyAction::Symbols => println!("key: SYMBOLS"),
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
