# swype-kbd

A swipe-typing (gesture/glide) on-screen keyboard for desktop Linux on Wayland,
targeting **Sway / wlroots**. Renders as a `zwlr_layer_shell_v1` surface, types
into other apps via `zwp_input_method_v2` + `zwp_virtual_keyboard_v1`, and
decodes word gestures with a SHARK2-style shape-matching decoder.

See [`ARCHITECTURE.md`](ARCHITECTURE.md) for the design and milestone plan.

## Status

**Milestone 4 — gesture typing.** Swiping a word is wired end to end: the pointer
trace is captured (with a live trail), decoded on release, and the top candidate
is committed with a trailing space; a suggestion bar shows alternates, and
tapping one replaces the committed word. Taps still type single characters. A
press is classified as tap vs. gesture by travel distance.

Earlier:
- **Milestone 3** — SHARK2 decoder in `swype-decoder` (shape + location channels,
  Zipfian prior, start/end pruning). Synthetic perturbation harness: top-1 94%,
  top-3 100% over the embedded list.
- **Milestone 2** — tap typing via `zwp_input_method_v2` (`commit_string`) with a
  `zwp_virtual_keyboard_v1` fallback (embedded US xkb keymap); Backspace/Enter as
  real key events.
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
