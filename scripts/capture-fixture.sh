#!/usr/bin/env bash
# Capture this machine as a sancho system fixture (eval/fixtures/systems/<name>).
#
# Read-only: runs only query commands and reads files; writes only into the new
# fixture directory. By default it captures the minimum: profile facts and HID
# devices (names and vendor:product IDs). --full adds the unit list, installed
# packages, network interfaces, browsers and group names, which describe what
# is installed on the machine. Serials, MACs, usernames and group members are
# never copied. Review the result before committing, and keep only what a test
# needs.
#
# Usage: scripts/capture-fixture.sh <name> [--full] [--with-sync-db]
set -euo pipefail
export LC_ALL=C

name=${1:?usage: $0 <name> [--full] [--with-sync-db]}
shift
full=""; sync=""
for arg in "$@"; do
  case $arg in
    --full) full=1 ;;
    --with-sync-db) full=1; sync=1 ;;
    *) echo "unknown option: $arg" >&2; exit 2 ;;
  esac
done
out="$(dirname "$0")/../eval/fixtures/systems/$name"
[[ -e $out ]] && { echo "$out already exists" >&2; exit 1; }
mkdir -p "$out/root" "$out/queries"
r="$out/root"

put() { mkdir -p "$(dirname "$r$1")"; cat > "$r$1"; }
mark() { mkdir -p "$(dirname "$r$1")"; : > "$r$1"; }

# --- profile facts ---
for f in /etc/os-release /usr/lib/os-release; do [[ -r $f ]] && put "$f" < "$f"; done
for d in /run/systemd/system /run/openrc /run/runit /run/dinitctl /run/s6; do
  [[ -e $d ]] && mark "$d/.keep"
done
put /proc/1/comm < /proc/1/comm
for p in NetworkManager connmand iwd systemd-network pipewire pulseaudio; do
  pid=$(pgrep -x "$p" | head -n1 || true)
  [[ -n $pid ]] && echo "$p" | put "/proc/$pid/comm"
done
awk '$2 == "/" { print "rootdev / " $3 " rw 0 0" }' /proc/self/mounts | put /proc/self/mounts
for prog in paru yay snapper timeshift; do
  if command -v "$prog" > /dev/null; then
    printf '#!/bin/sh\n' | put "/usr/bin/$prog"; chmod +x "$r/usr/bin/$prog"
  fi
done
for f in /etc/snapper/configs/root /etc/timeshift/timeshift.json \
         /boot/loader/loader.conf /efi/loader/loader.conf /boot/efi/loader/loader.conf \
         /boot/limine.conf /boot/limine/limine.conf /boot/EFI/limine/limine.conf \
         /efi/limine.conf /etc/default/limine /boot/EFI/refind/refind.conf \
         /efi/EFI/refind/refind.conf /boot/refind_linux.conf \
         /boot/grub/grub.cfg /etc/default/grub; do
  [[ -e $f ]] && mark "$f"
done

# --- resolver sources ---
# udev: only hid/hidraw/input records, only the properties resolvers read.
udevadm info --export-db | awk '
  /^P: / { path = $0; keep = ""; props = ""; next }
  /^E: (SUBSYSTEM|HID_ID|HID_NAME|ID_INPUT_[A-Z]+)=/ { props = props $0 "\n" }
  /^E: SUBSYSTEM=(hid|hidraw|input)$/ { keep = 1 }
  /^$/ { if (keep) printf "%s\n%s\n", path, props; path = ""; keep = "" }
' > "$out/queries/udev-db.txt"

{
  echo "PATH=/usr/local/bin:/usr/bin"
  echo "USER=user"
  [[ -n ${XDG_SESSION_TYPE:-} ]] && echo "XDG_SESSION_TYPE=$XDG_SESSION_TYPE"
  [[ -n ${WAYLAND_DISPLAY:-} ]] && echo "WAYLAND_DISPLAY=$WAYLAND_DISPLAY"
  [[ -n ${DISPLAY:-} ]] && echo "DISPLAY=$DISPLAY"
} > "$out/env"

if [[ -z $full ]]; then
  echo "Captured $out (minimal) — review it before committing."
  exit 0
fi

systemctl list-unit-files --no-legend --no-pager > "$out/queries/unit-files.txt"
pacman -Q > "$out/queries/installed-packages.txt"
[[ -n $sync ]] && pacman -Sl > "$out/queries/sync-packages.txt"

for dev in /sys/class/net/*; do
  n=$(basename "$dev")
  [[ -e $dev/device ]] && mark "/sys/class/net/$n/device"
  [[ -e $dev/wireless ]] && mark "/sys/class/net/$n/wireless"
  put "/sys/class/net/$n/type" < "$dev/type"
done

for f in /usr/share/applications/*.desktop; do
  grep -q '^Categories=.*WebBrowser' "$f" || continue
  awk '/^\[/ { main = ($0 == "[Desktop Entry]") } main && /^(\[Desktop Entry\]|Name=|Categories=|Hidden=|NoDisplay=)/' "$f" \
    | put "$f"
done

# Accounts: group names without members; one regular user named "user".
# Groups of regular users (GID 1000-59999) are personal names; replace them.
{ cut -d: -f1,3 /etc/group | awk -F: '$2 < 1000 || $2 >= 60000 { print $1 ":x:" $2 ":" }'
  echo "user:x:1000:"; } | put /etc/group
echo "user:x:1000:1000::/home/user:/bin/bash" | put /etc/passwd

echo "Captured $out — review it before committing."
