# swype-kbd

A swipe-typing (gesture/glide) on-screen keyboard for desktop Linux on Wayland,
targeting **Sway / wlroots**. Renders as a `zwlr_layer_shell_v1` surface, types
into other apps via `zwp_input_method_v2` + `zwp_virtual_keyboard_v1`, and
decodes word gestures with a SHARK2-style shape-matching decoder.

See [`ARCHITECTURE.md`](ARCHITECTURE.md) for the design and milestone plan.

## Status

**MVP complete (milestones 1–5).** A working swipe-typing keyboard: it docks to
the bottom of the screen, types into focused apps, and decodes word gestures.

- **M5 — real dictionary + latency.** Loads a ~50k-word unigram frequency list
  (Norvig counts, see `data/`) at runtime; 49,945 templates build in ~0.25 s.
  Decode latency mean ~3 ms, p99 ~7 ms — under the 30 ms budget. Override the
  list with `SWYPE_DICT`.
- **M4 — gesture typing.** Pointer trace captured with a live trail, decoded on
  release, top candidate committed + space; a suggestion bar shows alternates,
  tap to replace. Taps still type single characters.
- **M3 — SHARK2 decoder** (`swype-decoder`): shape + location channels, frequency
  prior, start/end pruning. Synthetic perturbation harness: top-1 94%, top-3
  100% on common words.
- **M2 — tap typing** via `zwp_input_method_v2` (`commit_string`) with a
  `zwp_virtual_keyboard_v1` fallback (embedded US xkb keymap); Backspace/Enter as
  real key events.
- **M1 — layer-shell surface**, software-rendered QWERTY, pointer hit-testing.

### Possible next steps (post-MVP)

- Shift/symbols layers (the keys exist but are inert), numbers row.
- Trail smoothing / per-point velocity weighting in the decoder.
- Damage-tracked partial repaints instead of full-surface redraw per motion.
- Per-user learning of the frequency prior.

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
