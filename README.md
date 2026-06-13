# swype-kbd

A swipe-typing (gesture/glide) on-screen keyboard for desktop Linux on Wayland,
targeting **Sway / wlroots**. It renders as a `zwlr_layer_shell_v1` surface,
types into other apps via `zwp_input_method_v2` + `zwp_virtual_keyboard_v1`, and
decodes word gestures with a SHARK2-style shape-matching decoder.

See [`ARCHITECTURE.md`](ARCHITECTURE.md) for the design and milestone history.

## Features

- **Swipe typing** — glide across a word's letters; on release a SHARK2-style
  decoder (shape + location + endpoint channels, frequency prior, and
  velocity-weighted matching) commits the most likely word plus a space. A
  suggestion bar shows alternates — tap to replace.
- **Tap typing** with predictive **word completions** in the suggestion bar.
- **Next-word prediction** — a learned bigram model offers the word you usually
  type next, right after you finish a word.
- **Pointer and touch** input (`wl_pointer` and `wl_touch`).
- **Input layers** — Shift (one-shot → caps-lock → off) and two symbol pages
  cover the full ASCII set, through both the input-method and virtual-keyboard
  channels.
- **Smart typing** — gesture words commit with a trailing space; punctuation
  tucks against the word (`hello .` → `hello.`); the first letter after `. ! ?`
  or Enter auto-capitalizes.
- **Learns your tendencies** — every word you accept (swipe, complete, or tap
  out) gently rises in frequency, and corrections converge a confusable pair.
  Words you type that aren't in the dictionary join a **personal dictionary**
  after a few uses, becoming swipeable and completable like any other word.
  All of it persists under `$XDG_STATE_HOME/swype-kbd/` (`learned.txt`,
  `personal.txt`, `bigrams.txt`).
- **One-tap undo** — Backspace right after a committed word removes it whole.
- **Polished UI** — anti-aliased TrueType labels, rounded keys with depth, a key
  pop-up preview, and a glowing gesture trail, all from a dependency-light
  software rasterizer with damage-tracked repaints.
- **Real dictionary, low latency** — loads a ~50k-word unigram frequency list at
  runtime (49,945 templates build in ~0.25 s); decode mean ~3 ms, p99 ~7 ms,
  under a 30 ms budget. Synthetic accuracy ≈98% top-1, 100% top-3 on common
  words. Override the list with `SWYPE_DICT`.

## Requirements

- A **wlroots-based Wayland compositor** — Sway, Hyprland, river, Wayfire, … —
  exposing `zwlr_layer_shell_v1`, `zwp_input_method_v2`, and
  `zwp_virtual_keyboard_v1`. It does **not** run on GNOME, KDE Plasma, or X11.
- A Rust toolchain to build (stable).

## Build & test

```sh
cargo build --release
cargo test          # decoder + app unit tests
```

## Run

Launch it from a terminal **inside** your Sway/wlroots session (so
`WAYLAND_DISPLAY` is set):

```sh
cargo run --release -p swype-kbd
# or: ./target/release/swype-kbd
```

A keyboard docks to the bottom of the screen and types into whatever app has
focus. Tap keys to type, or glide across letters to swipe a word; the suggestion
bar offers alternates and completions. Decode/debug traces print to stdout. Exit
with Ctrl-C.

The dictionary is found via `$SWYPE_DICT`, then a path next to the binary, then
`./data/`; a small embedded list is the fallback so the binary also works
standalone.

## Layout

- `crates/decoder` (`swype-decoder`) — the pure, Wayland-free gesture decoder and
  shared keyboard geometry. Fully unit-tested.
- `crates/app` (`swype-kbd`) — the Wayland layer-shell binary: software
  rasterizer (anti-aliased text, rounded keys), pointer/touch hit-testing, and
  text injection.

## Ideas / not yet done

- Multi-touch (two-thumb) gesturing and a number row.
- Theming and sizing config (height, colors, light theme, landscape).
- Packaging for distribution (release tarball / distro packages / autostart).
- A cached static-background render pass, if the per-motion repaint is ever too
  heavy on low-end hardware.

## Credits

Bundles **DejaVu Sans** for UI text; see
[`crates/app/assets/DejaVuSans.LICENSE`](crates/app/assets/DejaVuSans.LICENSE).
