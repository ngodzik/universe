#!/bin/sh
# Installs, once, the Chrome (CEF) the App view shows its page with, where a
# build run from this folder finds it: ~/.local/share/cef. On Linux a build
# downloads it by itself; this is needed on macOS, where Chrome runs from a
# framework of its own and draws its pages in a process of its own.
set -eu

CEF_VERSION="154.5.0+154.0.34"
RENDER_PROCESS_VERSION="0.14.0"
DIR="$HOME/.local/share/cef"

case "$(uname -s)" in
Darwin)
    # Its build needs both, to build CEF's wrapper.
    if ! command -v cmake >/dev/null 2>&1 || ! command -v ninja >/dev/null 2>&1; then
        brew install cmake ninja
    fi
    cargo install --locked "export-cef-dir@$CEF_VERSION"
    export-cef-dir --force "$DIR"
    # Kept with its symbols, so that a crash in it names where.
    CARGO_PROFILE_RELEASE_STRIP=false cargo install --locked \
        "bevy_cef_debug_render_process@$RENDER_PROCESS_VERSION"
    cp "$HOME/.cargo/bin/bevy_cef_debug_render_process" \
        "$DIR/Chromium Embedded Framework.framework/Libraries/"
    echo "Chrome for universe is in $DIR."
    ;;
*)
    echo "Nothing to do here: a build downloads Chrome for universe by itself."
    echo "Setting CEF_PATH=$DIR keeps one download for every build folder."
    ;;
esac
