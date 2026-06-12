# swype-kbd

A swipe-typing (gesture/glide) on-screen keyboard for desktop Linux on Wayland,
targeting **Sway / wlroots**. Renders as a `zwlr_layer_shell_v1` surface, types
into other apps via `zwp_input_method_v2` + `zwp_virtual_keyboard_v1`, and
decodes word gestures with a SHARK2-style shape-matching decoder.

See [`ARCHITECTURE.md`](ARCHITECTURE.md) for the design and milestone plan.

## Status

**Milestone 3 — SHARK2 decoder.** The `swype-decoder` crate now ranks dictionary
words for a swipe: ideal templates from letter centroids, a shape channel
(scale/translation-invariant) and a location channel, a Zipfian frequency prior,
and a start/end pruning gate. A seeded synthetic perturbation harness (jitter,
corner-cutting, overshoot) measures accuracy — currently **top-1 94%, top-3
100%** at moderate perturbation over the embedded word list. Not yet wired to the
keyboard surface.

Earlier:
- **Milestone 2** — tap typing into the focused app via `zwp_input_method_v2`
  (`commit_string`) with a `zwp_virtual_keyboard_v1` fallback (embedded US xkb
  keymap); Backspace/Enter as real key events.
- **Milestone 1** — bottom-docked layer-shell surface, software-rendered QWERTY,
  pointer hit-testing.

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
