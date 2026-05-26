#!/usr/bin/env sh
set -eu

repo="barkure/bucketctl"
install_dir="${HOME}/.local/bin"
bin_name="bucketctl"
target="${install_dir}/${bin_name}"
action="${1:-install}"

remove_binary() {
  if [ ! -e "${target}" ]; then
    echo "not installed: ${target}"
    exit 0
  fi

  rm -f "${target}"
  echo "removed ${target}"
}

install_binary() {
  os="$(uname -s)"
  arch="$(uname -m)"

  case "${os}/${arch}" in
    Darwin/arm64)
      suffix="darwin-arm64.tar.gz"
      ;;
    Linux/x86_64)
      suffix="linux-amd64.tar.gz"
      ;;
    Linux/aarch64)
      suffix="linux-arm64.tar.gz"
      ;;
    *)
      echo "unsupported platform: ${os}/${arch}" >&2
      exit 1
      ;;
  esac

  api_url="https://api.github.com/repos/${repo}/releases/latest"
  asset_url="$(curl -fsSL "${api_url}" | grep -Eo "https://[^[:space:]\"]*${suffix}" | head -n 1)"

  if [ -z "${asset_url}" ]; then
    echo "failed to find release asset for ${suffix}" >&2
    exit 1
  fi

  tmp_dir="$(mktemp -d)"
  archive_path="${tmp_dir}/${bin_name}.tar.gz"

  cleanup() {
    rm -rf "${tmp_dir}"
  }

  trap cleanup EXIT INT TERM

  mkdir -p "${install_dir}"
  curl -fL -o "${archive_path}" "${asset_url}"
  tar -xzf "${archive_path}" -C "${tmp_dir}"
  install -m 755 "${tmp_dir}/${bin_name}" "${target}"

  echo "installed ${bin_name} to ${target}"
}

case "${action}" in
  install)
    install_binary
    ;;
  remove)
    remove_binary
    ;;
  *)
    echo "usage: install.sh [install|remove]" >&2
    exit 1
    ;;
esac
