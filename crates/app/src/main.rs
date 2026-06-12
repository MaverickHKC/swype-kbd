//! swype-kbd — milestone 1.
//!
//! A `zwlr_layer_shell_v1` client that docks a software-rendered QWERTY keyboard
//! to the bottom of the screen and prints the key hit by each pointer press to
//! stdout. No text injection or gesture decoding yet (see ARCHITECTURE.md §5).

mod font;
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
    Connection, QueueHandle,
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

    let mut app = App {
        registry_state: RegistryState::new(&globals),
        seat_state: SeatState::new(&globals, &qh),
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
            let cap = &self.keyboard.caps()[i];
            print_action(cap.action);
            self.pressed = Some(i);
            self.draw(qh);
        }
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
