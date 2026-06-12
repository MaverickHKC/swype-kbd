# swype-kbd

A swipe-typing (gesture/glide) on-screen keyboard for desktop Linux on Wayland,
targeting **Sway / wlroots**. Renders as a `zwlr_layer_shell_v1` surface, types
into other apps via `zwp_input_method_v2` + `zwp_virtual_keyboard_v1`, and
decodes word gestures with a SHARK2-style shape-matching decoder.

See [`ARCHITECTURE.md`](ARCHITECTURE.md) for the design and milestone plan.

## Status

**Milestone 1 — layer-shell skeleton.** A bottom-docked keyboard surface that
software-renders a static QWERTY layout and prints the key hit by each pointer
press to stdout. No text injection or gesture decoding yet.

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
