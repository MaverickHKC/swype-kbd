# swype-kbd

A swipe-typing (gesture/glide) on-screen keyboard for desktop Linux on Wayland,
targeting **Sway / wlroots**. Renders as a `zwlr_layer_shell_v1` surface, types
into other apps via `zwp_input_method_v2` + `zwp_virtual_keyboard_v1`, and
decodes word gestures with a SHARK2-style shape-matching decoder.

See [`ARCHITECTURE.md`](ARCHITECTURE.md) for the design and milestone plan.

## Status

**Milestone 2 — tap typing.** Tapping a key types its character into the
focused application: text goes through `zwp_input_method_v2` (`commit_string`)
when a text field is active, and falls back to synthesized keystrokes via
`zwp_virtual_keyboard_v1` (with an embedded US xkb keymap) otherwise. Backspace
and Enter are real key events. No gesture decoding yet.

Earlier: **Milestone 1** — bottom-docked layer-shell surface, software-rendered
QWERTY, pointer hit-testing.

## Layout

- `crates/decoder` (`swype-decoder`) — the pure, Wayland-free gesture decoder
  and shared keyboard geometry. Fully unit-tested.
- `crates/app` (`swype-kbd`) — the Wayland layer-shell binary: software
  rasterizer, embedded bitmap font, pointer hit-testing.

## Build & test

```sh
cargo build --release
cargo test          # decoder + app unit tests
```

## Run (in a live Sway/wlroots session)

The binary needs a wlroots compositor with `zwlr_layer_shell_v1`. Run it from a
terminal **inside** your Sway session (so `WAYLAND_DISPLAY` is set):

```sh
cargo run --release -p swype-kbd
# or: ./target/release/swype-kbd
```

A keyboard docks to the bottom of the screen. Clicking a key prints e.g.
`key: 'a'` / `key: SPACE` to the terminal. Exit with Ctrl-C.
