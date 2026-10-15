#!/bin/sh
# Install one Cargo binary and a short-name symlink, without touching settings.
set -eu

if [ "$#" -gt 1 ]; then
    echo 'Usage: sh tools/install-dev.sh [INSTALL_ROOT]' >&2
    exit 2
fi
repo_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
install_root=${1:-${CARGO_INSTALL_ROOT:-${CARGO_HOME:-"$HOME/.cargo"}}}
ep_path=$install_root/bin/ep

# Never replace another application's command, including dangling symlinks.
if [ -e "$ep_path" ] || [ -L "$ep_path" ]; then
    if [ ! -L "$ep_path" ] || [ "$(readlink "$ep_path")" != editor-pro ]; then
        echo "Refusing to replace $ep_path; choose a different install root." >&2
        exit 1
    fi
fi

cargo install --locked --path "$repo_dir" --root "$install_root"
if [ ! -L "$ep_path" ]; then
    ln -s editor-pro "$ep_path"
fi
printf 'Installed editor-pro and ep. Add %s/bin to PATH.\n' "$install_root"
