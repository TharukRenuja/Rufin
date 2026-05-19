#!/usr/bin/env bash
set -euo pipefail

readonly app_id="io.github.screwys.Rufin"
readonly bundle_name="${app_id}.flatpak"
readonly bundle_url="https://github.com/screwys/Rufin/releases/latest/download/${bundle_name}"
readonly service_name="rufin-flatpak-update"
readonly user_bin_dir="${HOME}/.local/bin"
readonly user_systemd_dir="${XDG_CONFIG_HOME:-${HOME}/.config}/systemd/user"

install_rufin() {
  local tmp

  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' EXIT

  curl -L --fail -o "${tmp}/${bundle_name}" "${bundle_url}"
  flatpak install --user --or-update --noninteractive --bundle "${tmp}/${bundle_name}"
}

enable_daily_updates() {
  mkdir -p "${user_bin_dir}" "${user_systemd_dir}"

  install -m 0755 /dev/stdin "${user_bin_dir}/${service_name}" <<EOF
#!/usr/bin/env bash
set -euo pipefail

tmp="\$(mktemp -d)"
trap 'rm -rf "\${tmp}"' EXIT

curl -L --fail -o "\${tmp}/${bundle_name}" "${bundle_url}"
flatpak install --user --or-update --noninteractive --bundle "\${tmp}/${bundle_name}"
EOF

  install -m 0644 /dev/stdin "${user_systemd_dir}/${service_name}.service" <<EOF
[Unit]
Description=Check for Rufin Flatpak updates

[Service]
Type=oneshot
ExecStart=%h/.local/bin/${service_name}
EOF

  install -m 0644 /dev/stdin "${user_systemd_dir}/${service_name}.timer" <<EOF
[Unit]
Description=Check for Rufin Flatpak updates daily

[Timer]
OnCalendar=daily
Persistent=true
Unit=${service_name}.service

[Install]
WantedBy=timers.target
EOF

  if command -v systemctl >/dev/null 2>&1 &&
    systemctl --user daemon-reload &&
    systemctl --user enable --now "${service_name}.timer"; then
    printf 'Enabled daily Rufin Flatpak update checks with %s.timer.\n' "${service_name}"
  else
    printf 'Created the systemd user units, but could not enable them automatically.\n'
    printf 'Run: systemctl --user daemon-reload && systemctl --user enable --now %s.timer\n' "${service_name}"
  fi
}

maybe_enable_daily_updates() {
  local answer

  if [[ ! -t 0 ]]; then
    return
  fi

  read -r -p "Create a systemd user service to check for Rufin updates daily? [y/N] " answer || answer=""

  case "${answer,,}" in
    y | yes)
      enable_daily_updates
      ;;
    *)
      printf 'Daily Rufin Flatpak update checks were not enabled.\n'
      ;;
  esac
}

install_rufin
maybe_enable_daily_updates
