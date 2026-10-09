#!/usr/bin/env fish
# Build and install Jobflick to Cargo's normal binary directory.
# Service management and desktop keybindings are deliberately separate.
set -l script_dir (dirname (status --current-filename))
set -l repo (realpath "$script_dir/..")
cd "$repo"; or exit 1

cargo install --path . --force; or exit 1
echo "Installed Jobflick with Cargo. No desktop settings or services were changed."
