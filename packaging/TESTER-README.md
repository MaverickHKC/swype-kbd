# swype-kbd — tester build

A Wayland swipe-typing on-screen keyboard. Swipe across the keys to type a word,
or tap keys individually. It learns your vocabulary and next-word habits over time.

## Requirements

- A **wlroots-based** Wayland compositor: **Sway, Hyprland, river, or Wayfire**.
  GNOME (Mutter) and KDE (KWin) are **not** supported — they lack the required
  protocols (`zwlr_layer_shell_v1`, `zwp_input_method_v2`,
  `zwp_virtual_keyboard_v1`).
- **x86_64 Linux** with **glibc ≥ 2.34** (any distro from ~2021 onward).
  Check with: `ldd --version`
- That's it — no Rust toolchain, no GPU. The font and UI are bundled in the binary.

If your glibc is older or your machine isn't x86_64, use **Build from source** below.

## Run

```sh
./run.sh
```

The launcher preflight-checks that your compositor exposes the needed protocols
(it uses `wayland-info` if installed), then starts the keyboard docked at the
bottom of the screen. Open any text field and start swiping.

To stop it: `pkill -x swype-kbd` (or Ctrl-C in the terminal you launched it from).

## What to try

- **Swipe typing** — drag from key to key in one motion, lift to commit the top
  guess. The suggestion bar shows alternates; tap one to swap.
- **Tap typing** — tap keys individually; the bar shows word completions.
- **It personalizes** — words you use get more likely; tap-typed words not in the
  dictionary get added after a few uses; after you commit a word, the bar predicts
  a likely next word. State persists in
  `${XDG_STATE_HOME:-~/.local/state}/swype-kbd/`.
- **Undo** — Backspace right after a committed word removes the whole word.

## Troubleshooting

- **Exits immediately / "missing protocol"** — your compositor isn't wlroots-based
  or doesn't expose layer-shell + input-method-v2 + virtual-keyboard-v1. swype-kbd
  can't run there.
- **"WAYLAND_DISPLAY is not set"** — run it from inside your graphical session, not
  a bare SSH shell.
- **Glibc error** (`version GLIBC_2.34 not found`) — your distro is older than the
  prebuilt binary supports; build from source instead.
- **Logs** — run `./run.sh 2>&1 | tee /tmp/swype.log` and share that file.

## Build from source

If the prebuilt binary won't run (old glibc, non-x86_64), build it yourself:

```sh
# needs Rust ≥ 1.80 (https://rustup.rs)
git clone <repo-url> swype-kbd && cd swype-kbd
cargo build --release
SWYPE_DICT="$PWD/data/words_50k.txt" ./target/release/swype-kbd
```

## Credits

Bundles the DejaVu Sans font — see `DejaVuSans.LICENSE`.
