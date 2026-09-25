#!/usr/bin/env bash
# Android Tools — Arch Linux installer.
# Usage: curl -fsSL https://raw.githubusercontent.com/Kismeria/Android-Tools/main/install.sh | bash
set -euo pipefail

PKG_URL="https://github.com/Kismeria/Android-Tools/releases/latest/download/android-tools-gui-x86_64.pkg.tar.zst"

if ! command -v pacman >/dev/null; then
  echo "This installer is for Arch Linux and derivatives (pacman)." >&2
  exit 1
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
echo "Android Tools: downloading..."
curl -fL --progress-bar -o "$tmp/android-tools-gui.pkg.tar.zst" "$PKG_URL"
sudo pacman -U --needed --noconfirm "$tmp/android-tools-gui.pkg.tar.zst"
# Phone as a microphone: pactl/pacat talk to PipeWire or PulseAudio.
command -v pactl >/dev/null || sudo pacman -S --needed --noconfirm libpulse

# Optional: phone as a webcam (v4l2loopback via DKMS needs headers for the running kernel).
answer="n"
if [ -r /dev/tty ]; then
  read -r -p "Install webcam support (v4l2loopback)? [y/N] " answer < /dev/tty || true
fi
if [[ "$answer" =~ ^[YyДд] ]]; then
  kernel_pkg="$(pacman -Qqo "/usr/lib/modules/$(uname -r)/vmlinuz" 2>/dev/null || echo linux)"
  sudo pacman -S --needed --noconfirm "${kernel_pkg}-headers" v4l2loopback-dkms v4l2loopback-utils
fi

echo "Done. Start it from the app menu or run: android-tools-gui"
