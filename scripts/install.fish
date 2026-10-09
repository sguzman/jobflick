#!/usr/bin/env fish
# Install Jobflick from a checkout without changing desktop keybindings.
set -l script_dir (dirname (status --current-filename))
set -l repo (realpath "$script_dir/..")
cd "$repo"; or exit 1

for executable in cargo fish wl-copy wl-paste notify-send
    if not type -q $executable
        echo "Missing dependency: $executable"
        echo "On Arch/EndeavourOS, install the appropriate Rust, Fish, wl-clipboard, and libnotify packages."
        exit 1
    end
end

echo "Building Jobflick..."
cargo build --release; or exit 1
# Replace the executable atomically. A running daemon can keep using the old
# inode while newly launched HUDs use the new build; no job interruption.
set -l install_dir "$HOME/.local/bin"
mkdir -p "$install_dir"; or exit 1
set -l staged (mktemp "$install_dir/.jobflick.XXXXXXXX"); or exit 1
if not install -m755 target/release/jobflick "$staged"
    rm -f -- "$staged"
    exit 1
end
if not mv -f -- "$staged" "$install_dir/jobflick"
    rm -f -- "$staged"
    exit 1
end
echo "Installed: $install_dir/jobflick"

if type -q systemctl
    install -Dm644 packaging/jobflick.service "$HOME/.config/systemd/user/jobflick.service"; or exit 1
    if systemctl --user daemon-reload
        if systemctl --user enable --now jobflick.service
            echo "Enabled Jobflick background service."
        else
            echo "User service could not start; Jobflick can also launch its daemon on demand."
        end
    else
        echo "systemd --user unavailable; Jobflick can launch its daemon on demand."
    end
end

echo
echo "Add these Hyprland bindings if the keys are free:"
echo "  bind = SUPER, RETURN, exec, $HOME/.local/bin/jobflick submit --clipboard"
echo "  bind = SUPER, J, exec, $HOME/.local/bin/jobflick hud"
echo "And make the HUD float using the window rule documented in README.md."
echo "Setup complete."
