#!/usr/bin/env bash
# swype-kbd launcher — preflight-checks the Wayland environment, then runs the
# keyboard from this directory so it finds words_50k.txt sitting next to it.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
bin="$here/swype-kbd"

if [[ ! -x "$bin" ]]; then
    echo "error: $bin not found or not executable" >&2
    exit 1
fi

# --- Wayland session checks -------------------------------------------------
if [[ -z "${WAYLAND_DISPLAY:-}" ]]; then
    echo "error: WAYLAND_DISPLAY is not set." >&2
    echo "       Run this from inside your Wayland session (Sway/Hyprland/river/Wayfire)," >&2
    echo "       not over a plain SSH shell." >&2
    exit 1
fi

# --- Protocol preflight (best-effort; needs wayland-info) -------------------
# swype-kbd requires a wlroots compositor exposing these three protocols.
required=(zwlr_layer_shell_v1 zwp_input_method_manager_v2 zwp_virtual_keyboard_manager_v1)
if command -v wayland-info >/dev/null 2>&1; then
    globals="$(wayland-info 2>/dev/null || true)"
    missing=()
    for p in "${required[@]}"; do
        grep -q "$p" <<<"$globals" || missing+=("$p")
    done
    if (( ${#missing[@]} )); then
        echo "error: your compositor is missing required Wayland protocol(s):" >&2
        printf '         %s\n' "${missing[@]}" >&2
        echo "       swype-kbd needs a wlroots compositor (Sway, Hyprland, river, Wayfire)." >&2
        echo "       GNOME (Mutter) and KDE (KWin) are not supported." >&2
        exit 1
    fi
else
    echo "note: 'wayland-info' not found — skipping protocol preflight." >&2
    echo "      If the keyboard exits immediately, your compositor likely lacks" >&2
    echo "      layer-shell / input-method-v2 / virtual-keyboard-v1 support." >&2
fi

# --- Run --------------------------------------------------------------------
# current_exe path-search picks up ./words_50k.txt automatically; SWYPE_DICT
# can override it. Personalization state is written to
# ${XDG_STATE_HOME:-~/.local/state}/swype-kbd/.
exec "$bin" "$@"
