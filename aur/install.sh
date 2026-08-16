#!/usr/bin/env bash
# Build the split packages, then install abs (always) and AbsGui when the user
# wants it. First install (no ~/.config/abs/abs.toml and no remembered pref)
# asks. Later reinstalls / self-updates reuse install_absgui from abs.toml or
# install-prefs.toml.
set -euo pipefail
cd "$(dirname "$0")"

makepkg -s

shopt -s nullglob
abs_pkgs=(abs-[0-9]*.pkg.tar.*)
gui_pkgs=(absgui-*.pkg.tar.*)
full_pkgs=(abs-full-*.pkg.tar.*)
if ((${#abs_pkgs[@]} == 0)); then
  echo "No abs-*.pkg.tar.* artifacts. makepkg -s failed?" >&2
  exit 1
fi

config_dir="${XDG_CONFIG_HOME:-$HOME/.config}/abs"
user_toml="$config_dir/abs.toml"
prefs="$config_dir/install-prefs.toml"

# 0 = true, 1 = false, 2 = missing
bool_from_file() {
  local file="$1"
  [[ -f "$file" ]] || return 2
  local line
  line="$(grep -E '^[[:space:]]*install_absgui[[:space:]]*=' "$file" | tail -n1 || true)"
  [[ -n "$line" ]] || return 2
  case "${line#*=}" in
    *true* | *True* | *TRUE*) return 0 ;;
    *false* | *False* | *FALSE*) return 1 ;;
    *) return 2 ;;
  esac
}

want_gui=
status=2
bool_from_file "$user_toml" && status=0 || status=$?
if [[ "$status" -eq 2 ]]; then
  bool_from_file "$prefs" && status=0 || status=$?
fi
case "$status" in
  0) want_gui=1 ;;
  1) want_gui=0 ;;
  *)
    if [[ -f "$user_toml" ]]; then
      if pacman -Q absgui &>/dev/null; then
        want_gui=1
      else
        want_gui=0
      fi
    elif [[ -n "${ABS_INSTALL_GUI:-}" ]]; then
      case "${ABS_INSTALL_GUI}" in
        0 | n | N | no | No | false | FALSE) want_gui=0 ;;
        *) want_gui=1 ;;
      esac
    elif [[ -t 0 ]]; then
      printf 'Install AbsGui (graphical interface)? [Y/n] '
      read -r ans || ans=
      case "${ans:-Y}" in
        n | N | no | No) want_gui=0 ;;
        *) want_gui=1 ;;
      esac
    else
      want_gui=1
    fi
    ;;
esac

mkdir -p "$config_dir"
if [[ "$want_gui" -eq 1 ]]; then
  printf 'install_absgui = true\n' >"$prefs"
else
  printf 'install_absgui = false\n' >"$prefs"
fi
chmod 600 "$prefs" 2>/dev/null || true

pkgs=("${abs_pkgs[-1]}")
if [[ "$want_gui" -eq 1 ]]; then
  if ((${#gui_pkgs[@]} == 0 || ${#full_pkgs[@]} == 0)); then
    echo "AbsGui packages were not built (need absgui-*.pkg.tar.* and abs-full-*.pkg.tar.*)." >&2
    exit 1
  fi
  pkgs+=("${gui_pkgs[-1]}" "${full_pkgs[-1]}")
fi

echo "Installing: ${pkgs[*]}"
sudo pacman -U "${pkgs[@]}"
