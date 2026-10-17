#!/usr/bin/env bash
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

install -Dm755 "$here/now.py" "$HOME/.local/bin/now"
mkdir -p "$HOME/.config/systemd/user"
install -m644 "$here/systemd/now-notify.service" "$HOME/.config/systemd/user/now-notify.service"
install -m644 "$here/systemd/now-notify.timer" "$HOME/.config/systemd/user/now-notify.timer"
if command -v systemctl >/dev/null; then systemctl --user daemon-reload || true; fi

if [[ "${1:-}" == "--omarchy" ]]; then
  plugin_id="io.github.godxgun.angela"
  dest="$HOME/.config/omarchy/plugins/$plugin_id"
  mkdir -p "$dest"
  for file in manifest.json BarWidget.qml Panel.qml; do
    install -m644 "$here/$file" "$dest/$file"
  done
  if command -v omarchy >/dev/null; then
    omarchy plugin validate "$dest"
    omarchy plugin enable "$plugin_id"
  else
    printf '%s\n' 'Omarchy CLI not found; plugin files copied but not enabled.'
  fi
  printf 'Omarchy plugin installed at %s\n' "$dest"
fi

printf '%s\n' 'Installed ~/.local/bin/now'
printf '%s\n' 'Next: now snapshot; now deadline add "Project" 2026-10-19T14:00'
printf '%s\n' 'Optional hourly alerts: systemctl --user enable --now now-notify.timer'
