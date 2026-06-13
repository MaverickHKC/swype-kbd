# swype-kbd — Architecture

A swipe-typing (gesture/glide) on-screen keyboard for desktop Linux on Wayland.
Renders as a `zwlr_layer_shell_v1` surface docked to the bottom of the screen,
injects text via `zwp_input_method_v2` (+ `zwp_virtual_keyboard_v1` for special
keys), and decodes word gestures with a SHARK2-style shape-matching decoder.

Target compositor: **Sway / wlroots**. Out of scope: GNOME, X11, tap-typing
autocorrect, non-English layouts.

---

## 1. Crate layout

This is a Cargo **workspace** with a hard wall between the Wayland app and the
gesture decoder:

```
swype-kbd/
├── Cargo.toml                # workspace root
├── ARCHITECTURE.md
├── crates/
│   ├── decoder/              # swype-decoder — PURE, no Wayland, no I/O
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── layout.rs     # QWERTY key geometry / centroids (shared model)
│   │   │   ├── trace.rs      # Point{x,y,t}, Trace, resample + normalize
│   │   │   ├── template.rs   # ideal-trace generation + cache (milestone 3)
│   │   │   └── decoder.rs    # SHARK2 scoring + pruning (milestone 3)
│   │   └── Cargo.toml
│   └── app/                  # swype-kbd — the binary
│       ├── src/
│       │   ├── main.rs       # Wayland wiring (layer-shell, seat, shm)
│       │   ├── render.rs     # software rasterizer over a wl_shm buffer
│       │   ├── font.rs       # embedded 5x7 bitmap font (zero-dep)
│       │   └── keyboard.rs   # visual layout + hit-testing (app-side)
│       └── Cargo.toml
└── data/
    └── words_50k.txt         # unigram frequency list (milestone 5)
```

**Why the decoder is a separate crate with no Wayland deps:** it is the part
worth unit-testing exhaustively. It takes `Trace`s in and returns ranked
`Candidate`s; it never touches a socket or a pixel. Tests feed synthetic
(programmatically perturbed) traces for known words and assert top-1/top-3
accuracy. The `layout` module lives in the decoder because both sides need the
same key centroids — the decoder to generate ideal traces, the app to render
and hit-test. The app depends on the decoder, never the reverse.

---

## 2. Rendering choice — software raster to a wl_shm buffer

**Decision: raw software rendering into a `wl_shm` (shared-memory) buffer; an
embedded 5x7 bitmap font for labels. No GPU, no GL, no text-shaping crate.**

Justification:
- **The VM has no GPU** (software rendering / llvmpipe). A GL/Vulkan path buys
  nothing and adds an llvmpipe dependency that is exactly the slow path we'd be
  rendering through anyway.
- **A keyboard is rectangles and glyphs.** The entire UI is filled rounded-ish
  rects, a few lines for the gesture trail, and short ASCII labels. That is a
  few hundred `fill_rect`/`blit_glyph` calls per frame — trivial on CPU, and it
  only repaints on state change (key press, gesture update), not continuously.
- **`wl_shm` is the lowest-friction path** with `smithay-client-toolkit`: its
  `Shm` + `SlotPool` helpers hand us a `&mut [u8]` canvas and a `wl_buffer`. We
  write `Argb8888` pixels (little-endian byte order B,G,R,A) directly.
- **Zero font dependency:** an embedded 5x7 bitmap font (authored in-repo,
  covers A–Z, 0–9, and the punctuation we render) keeps the dependency surface
  tiny and is more than legible for single-character key caps. If we later want
  anti-aliased text we can add `fontdue` (pure Rust) behind the same `Canvas`
  API — but we ask before pulling it in.

Buffer format is `Argb8888`. We keep a single back buffer sized to the surface
and repaint on demand (damage tracking can come later).

---

## 3. Wayland protocol flow

The app is simultaneously **a layer-shell client** (it owns an on-screen
surface) and **an input method** (it injects text into whatever app is focused).
These are independent connections over the same `wl_display`.

### 3.1 Globals we bind
| Global | Source | Purpose |
|---|---|---|
| `wl_compositor` | core | create the keyboard `wl_surface` |
| `wl_shm` | core | shared-memory pixel buffers |
| `wl_seat` | core | pointer + touch input to our surface |
| `zwlr_layer_shell_v1` | wlr-protocols | dock our surface to the screen edge |
| `zwp_input_method_manager_v2` | wlr (input-method-v2) | become the seat's input method |
| `zwp_virtual_keyboard_manager_v1` | wlr | synthesize raw keystrokes |

### 3.2 Layer surface (the keyboard window)
1. Create a `wl_surface`, then `zwlr_layer_shell_v1.get_layer_surface` on the
   **`top`** layer (above normal windows, below true overlays).
2. `set_anchor(BOTTOM | LEFT | RIGHT)` and `set_size(0, height)` — width 0 means
   "span the output", height is our fixed keyboard height (~`output_h / 3`).
3. `set_exclusive_zone(height)` so tiled windows shrink to sit above us instead
   of being covered.
4. **`set_keyboard_interactivity(None)`** — critical. An OSK must never take
   `wl_keyboard` focus, or it would steal focus from the very app the user is
   typing into. We receive **pointer/touch** on our surface, nothing else.
5. `commit()`, wait for `configure`, `ack_configure`, attach the first buffer.

Because we never take keyboard focus, the focused application keeps its text
field active and its caret blinking while the user pokes our surface.

### 3.3 Text injection — two channels

**Channel A — `zwp_input_method_v2` (preferred, for text):**
- `zwp_input_method_manager_v2.get_input_method(seat)` → `zwp_input_method_v2`.
- The compositor sends us `activate` / `deactivate` as the user focuses/blurs a
  text field *in some other app*, plus `surrounding_text`, `text_change_cause`,
  and `content_type`. Each batch ends with `done`, carrying a `serial`.
- To type: `commit_string("hello ")` then `commit(serial)`. We can also
  `delete_surrounding_text(before, after)` — that is how a gesture replaces the
  in-progress word when the user taps an alternate in the suggestion bar.
- This is the clean path: it commits real Unicode text, not keycodes, and works
  regardless of physical keymap. It only functions while `active`.

**Channel B — `zwp_virtual_keyboard_v1` (fallback + special keys):**
- `zwp_virtual_keyboard_manager_v1.create_virtual_keyboard(seat)` →
  `zwp_virtual_keyboard_v1`.
- Upload an xkb keymap once (`keymap(fd, size)`); we ship a US-QWERTY keymap
  built with `xkbcommon`.
- Send `key(time, keycode, state)` (+ `modifiers`) to synthesize real key
  events. Used for **Backspace, Enter, Tab, arrows**, and as the typing path for
  apps that do **not** implement `text-input-v3` (so `activate` never fires).

**Selection logic:** if input-method is `active`, commit text via Channel A and
use Channel B only for non-text keys (Enter/Backspace/Tab). If input-method is
*not* active, fall back to synthesizing every character through Channel B.

### 3.4 Focus & event routing summary
- Our layer surface (interactivity `None`) → receives `wl_pointer` / `wl_touch`
  only. Hit-testing maps a press location to a key or starts a gesture.
- The user's real app → keeps `wl_keyboard` focus and text-input focus.
- The input-method object is **seat-global**: wlroots routes the active
  text-input client to us regardless of which window is focused. We don't track
  windows; we react to `activate`/`deactivate`.

---

## 4. Decoder design (SHARK2-style)

Reference: Kristensson & Zhai, *SHARK²: a large vocabulary shorthand writing
system for pen-based computers* (UIST 2004). Two parallel channels — **shape**
and **location** — combined with a **frequency prior**.

### 4.1 Data model
- `Point { x: f32, y: f32, t: u32 }` — keyboard-space coords (layout units from
  `layout.rs`) + millisecond timestamp.
- `Trace(Vec<Point>)` — a raw captured gesture or a generated ideal path.
- `KeyboardLayout` — letter → centroid `(x, y)` in layout units (key = 1.0 wide).

### 4.2 Templates (ideal traces), precomputed & cached
For each dictionary word, the ideal gesture is the polyline visiting the
centroid of each letter in order. We **resample to N = 100 equidistant points**
(arc-length resampling) so every template and the user trace are length-aligned.
Templates are generated once at startup and cached (`Vec<[f32; 2*N]>` plus the
word's start/end centroid and frequency). 50k words × 100 points × 2 f32 ≈ 40 MB
— acceptable; can be quantized later.

### 4.3 The two channels
Both gestures are first **resampled to N points**.

- **Shape channel:** translate each gesture so its centroid is at the origin and
  scale so its bounding box's longer side is 1 (scale-/translation-invariant).
  Shape distance = mean Euclidean distance between corresponding points. This
  matches the *form* of the swipe regardless of where/how big it was drawn.
- **Location channel:** in original (un-normalized) keyboard space, the mean
  Euclidean distance between corresponding points, gated by a tunnel radius.
  This anchors the shape to actual keys, disambiguating same-shape words (e.g.
  short two-key swipes).

`combined = α · shape_norm + (1 − α) · location_norm`, distances mapped to a
likelihood via `exp(−d² / 2σ²)`.

### 4.4 Frequency prior
A unigram frequency list (~50k words, public English list) gives `P(word)`. Final
score `∝ likelihood(shape,loc) · P(word)^β`. β tunes how much the language model
overrides the gesture. Top candidate is committed; ranks 2–4 fill the suggestion
bar.

### 4.5 Pruning (the 30 ms budget)
Full shape matching against 50k templates per release is too slow. Before
scoring:
1. **Start/end gate:** keep only words whose first-letter centroid is within
   radius `r` of the trace's first point **and** last-letter centroid within `r`
   of the last point. A gesture almost always begins on its first letter and
   ends on its last — this alone cuts 50k → typically < 500 candidates.
2. **Length band (optional):** drop candidates whose ideal arc-length is wildly
   different from the trace's.
Only survivors get the full shape+location pass. Templates are precomputed, the
hot loop is flat `f32` arithmetic over fixed-size arrays, and the candidate set
is small — comfortably under 30 ms. We measure it in a Criterion-style bench and
log the p99 in the app.

### 4.5b Personalization (learning over time)
The decoder's frequency prior is per-user adaptive, persisted under
`$XDG_STATE_HOME/swype-kbd/` and reloaded at startup:

- **Passive frequency learning** (`learned.txt`): every accepted word — swipe
  top-1, completion tap, prediction tap, or a known word tapped out — gets a
  small clamped boost to its log-frequency, so a person's real vocabulary floats
  up. A tap-to-replace correction rewards the chosen word and decays the rejected
  one (never below its base rank), so confusable pairs converge.
- **Personal dictionary** (`personal.txt`): a tap-typed word not in the base list
  is counted; after a small threshold it is inserted into the live decoder
  (`Decoder::add_word` builds a template and indexes it), seeded at the
  dictionary's 80th-frequency percentile (`Decoder::typical_ln_freq`, scale-robust
  across the embedded and 50k lists). It is then swipeable and completable. The
  threshold guards a one-off typo from becoming a word.
- **Next-word prediction** (`bigrams.txt`): a `prev → (next → count)` map learned
  from every committed word (any channel, including out-of-dictionary words). When
  no word is in progress, the most likely successors fill the suggestion bar; the
  context resets at sentence boundaries (`Enter`, `. ! ?`). A correction repoints
  the just-learned bigram from the rejected word onto the chosen one.

The decoder crate stays pure: it exposes `add_word` / `typical_ln_freq` /
`learn`, while all capture, thresholds, and persistence live in the app.

### 4.6 Test strategy
- **Synthetic corpus:** take each word's ideal trace and apply parametric
  perturbations — Gaussian jitter, corner-cutting (under-shoot at turns),
  overshoot past endpoints, speed variation in resampling. Assert top-1 / top-3
  accuracy across the corpus; track accuracy vs. perturbation strength as we tune
  α, β, σ, and the prune radius. All in-crate, no Wayland, fully deterministic.

---

## 5. Milestones

1. **Layer-shell skeleton** *(this commit)* — a bottom-docked layer surface that
   software-renders a static QWERTY layout and prints the hit key to stdout on
   pointer press. No text injection yet.
2. **Tap typing** — wire `zwp_input_method_v2` + `zwp_virtual_keyboard_v1`; a tap
   commits its character into the focused app; Backspace/Enter via virtual
   keyboard; suggestion bar stub.
3. **Decoder core** — `trace`, `template`, `decoder` modules; SHARK2 shape +
   location + prior + pruning; synthetic perturbation corpus + accuracy tests.
4. **Gesture integration** — capture the pointer trace on the surface, render the
   trail, decode on release, commit top candidate, show 3 alternates with
   tap-to-replace (`delete_surrounding_text` + `commit_string`).
5. **Polish** — load the real 50k unigram list, latency profiling/caching to hit
   the 30 ms p99 budget, visual tuning, basic settings.

Each milestone compiles, runs, and is committed before the next begins.
