#!/usr/bin/env bash
# Portable Steam / SteamVR path discovery (no machine-specific hardcoding).
# Override with STEAM_ROOT if needed.
#
# shellcheck shell=bash

_alvr_steam_roots() {
  local roots=()
  [[ -n "${STEAM_ROOT:-}" ]] && roots+=("$STEAM_ROOT")
  roots+=(
    "${HOME}/.steam/steam"
    "${HOME}/.steam/debian-installation"
    "${HOME}/.local/share/Steam"
    "${HOME}/.var/app/com.valvesoftware.Steam/.local/share/Steam"
  )
  local r
  for r in "${roots[@]}"; do
    [[ -n "$r" && -d "$r" ]] && printf '%s\n' "$r"
  done
}

# First Steam root that contains steamapps/common/SteamVR
alvr_find_steamvr_root() {
  local r
  while IFS= read -r r; do
    if [[ -d "$r/steamapps/common/SteamVR" ]]; then
      printf '%s\n' "$r/steamapps/common/SteamVR"
      return 0
    fi
  done < <(_alvr_steam_roots)
  return 1
}

alvr_find_steamvr_bin64() {
  local root
  root="$(alvr_find_steamvr_root)" || return 1
  printf '%s\n' "$root/bin/linux64"
}

alvr_find_steamvr_logs() {
  local r
  while IFS= read -r r; do
    if [[ -f "$r/logs/vrserver.txt" ]]; then
      printf '%s\n' "$r/logs"
      return 0
    fi
  done < <(_alvr_steam_roots)
  # Flatpak / alternate log locations
  local alt=(
    "${HOME}/.steam/steam/logs"
    "${HOME}/.local/share/Steam/logs"
  )
  for r in "${alt[@]}"; do
    [[ -f "$r/vrserver.txt" ]] && { printf '%s\n' "$r"; return 0; }
  done
  return 1
}

alvr_find_steamvr_settings() {
  local r
  while IFS= read -r r; do
    if [[ -f "$r/config/steamvr.vrsettings" ]]; then
      printf '%s\n' "$r/config/steamvr.vrsettings"
      return 0
    fi
  done < <(_alvr_steam_roots)
  return 1
}
