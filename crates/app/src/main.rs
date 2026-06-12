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

/// Requested keyboard height in pixels. Width spans the output.
const KBD_HEIGHT: u32 = 300;

// --- palette ---------------------------------------------------------------
const COL_BG: Color = Color::rgb(0x1a, 0x1c, 0x22);
const COL_KEY: Color = Color::rgb(0x33, 0x37, 0x42);
const COL_KEY_FN: Color = Color::rgb(0x26, 0x2a, 0x33);
const COL_KEY_PRESSED: Color = Color::rgb(0x4c, 0x8b, 0xf5);
const COL_LABEL: Color = Color::rgb(0xe6, 0xe8, 0xee);

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
        let stride = w as i32 * 4;
        let (buffer, canvas) = self
            .pool
            .create_buffer(w as i32, h as i32, stride, wl_shm::Format::Argb8888)
            .expect("failed to create buffer");

        {
            let mut cv = Canvas::new(canvas, w as usize, h as usize);
            cv.clear(COL_BG);

            let scale = key_label_scale(h);
            for (i, cap) in self.keyboard.caps().iter().enumerate() {
                let r = self.keyboard.rect_of(cap, w, h);
                // 2px gap between keys.
                let (kx, ky, kw, kh) = (r.x + 2, r.y + 2, r.w - 4, r.h - 4);
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

    fn on_press(&mut self, x: f64, y: f64, qh: &QueueHandle<Self>) {
        let hit = self
            .keyboard
            .hit_index(x as i32, y as i32, self.width, self.height);
        if let Some(i) = hit {
            let action = self.keyboard.caps()[i].action;
            self.handle_action(action);
            self.pressed = Some(i);
            self.draw(qh);
        }
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

    fn on_release(&mut self, qh: &QueueHandle<Self>) {
        if self.pressed.take().is_some() {
            self.draw(qh);
        }
    }
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
                PointerEventKind::Release { button, .. } if button == BTN_LEFT => {
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
