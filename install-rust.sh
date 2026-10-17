#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
cargo build --locked --release
install -Dm755 target/release/angela "${XDG_BIN_HOME:-$HOME/.local/bin}/angela"
mkdir -p "${XDG_DATA_HOME:-$HOME/.local/share}/applications"
cat > "${XDG_DATA_HOME:-$HOME/.local/share}/applications/angela.desktop" <<DESKTOP
[Desktop Entry]
Name=Angela
Comment=Local voice and schedule assistant
Exec=${XDG_BIN_HOME:-$HOME/.local/bin}/angela
Type=Application
Terminal=false
Categories=Office;Utility;
DESKTOP
printf 'Installed Angela. Run angela or open it from your launcher.\n'
