#!/bin/sh
set -eu

# This script is written to be as POSIX as possible
# so it works fine for all Unix-like operating systems

test_cmd() {
  command -v "$1" >/dev/null
}

# proxy version
lapce_new_ver="${1}"
# proxy directory
# eval to resolve '~' into proper user dir
eval lapce_dir="'${2}'"

case "${lapce_new_ver}" in
  v*)
    lapce_new_version=$(echo "${lapce_new_ver}" | cut -d'v' -f2)
    lapce_new_ver_tag="${lapce_new_ver}"
  ;;
  nightly*)
    lapce_new_version="${lapce_new_ver}"
    lapce_new_ver_tag=$(echo ${lapce_new_ver} | cut -d '-' -f1)
  ;;
  *)
    printf 'Unknown version\n'
    exit 1
  ;;
esac

if [ -e "${lapce_dir}/devforge" ]; then
  lapce_installed_ver=$("${lapce_dir}/devforge" --version | cut -d' ' -f2)

  printf '[DEBUG]: Current proxy version: %s\n' "${lapce_installed_ver}"
  printf '[DEBUG]: New proxy version: %s\n' "${lapce_new_version}"
  if [ "${lapce_installed_ver}" = "${lapce_new_version}" ]; then
    printf 'Proxy already exists\n'
    exit 0
  else
    printf 'Proxy outdated. Replacing proxy\n'
    rm "${lapce_dir}/devforge"
  fi
fi

for _cmd in tar gzip uname; do
  if ! test_cmd "${_cmd}"; then
    printf 'Missing required command: %s\n' "${_cmd}"
    exit 1
  fi
done

# Currently only linux/darwin are supported
case $(uname -s) in
  Linux) os_name=linux ;;
  Darwin) os_name=darwin ;;
  *)
    printf '[ERROR] unsupported os\n'
    exit 1
  ;;
esac

# Currently only amd64/arm64 are supported
case $(uname -m) in
  x86_64|amd64|x64) arch_name=x86_64 ;;
  arm64|aarch64) arch_name=aarch64 ;;
  # riscv64) arch_name=riscv64 ;;
  *)
    printf '[ERROR] unsupported arch\n'
    exit 1
  ;;
esac

devforge_url="https://github.com/bimacoding/DevForge/releases/download/${lapce_new_ver_tag}/devforge-proxy-${os_name}-${arch_name}.gz"
lapce_url="https://github.com/lapce/lapce/releases/download/nightly/lapce-proxy-${os_name}-${arch_name}.gz"
artifact="devforge-proxy-${os_name}-${arch_name}.gz"

printf 'Creating "%s"\n' "${lapce_dir}"
mkdir -p "${lapce_dir}"
cd "${lapce_dir}"

download_file() {
  _url="$1"
  _out="$2"
  if test_cmd 'curl'; then
    printf 'Downloading using curl: %s\n' "${_url}"
    curl --proto '=https' --tlsv1.2 -LfS -o "${_out}" "${_url}"
  elif test_cmd 'wget'; then
    printf 'Downloading using wget: %s\n' "${_url}"
    wget -O "${_out}" "${_url}"
  else
    printf 'curl/wget not found, failed to download proxy\n'
    return 1
  fi
}

if ! download_file "${devforge_url}" "${artifact}"; then
  printf 'DevForge proxy download failed; trying Lapce nightly fallback\n'
  if ! download_file "${lapce_url}" "${artifact}"; then
    printf 'All proxy downloads failed\n'
    exit 1
  fi
fi

printf 'Decompressing gzip\n'
gzip -df "${lapce_dir}/${artifact}"

printf 'Renaming proxy \n'
# gzip -d leaves either the DevForge or Lapce uncompressed name
if [ -e "${lapce_dir}/devforge-proxy-${os_name}-${arch_name}" ]; then
  mv -v "${lapce_dir}/devforge-proxy-${os_name}-${arch_name}" "${lapce_dir}/devforge"
elif [ -e "${lapce_dir}/lapce-proxy-${os_name}-${arch_name}" ]; then
  mv -v "${lapce_dir}/lapce-proxy-${os_name}-${arch_name}" "${lapce_dir}/devforge"
else
  # We forced -o to artifact name; gzip strips .gz → same stem
  stem=$(echo "${artifact}" | sed 's/\.gz$//')
  if [ -e "${lapce_dir}/${stem}" ]; then
    mv -v "${lapce_dir}/${stem}" "${lapce_dir}/devforge"
  else
    printf 'Downloaded proxy binary not found after decompress\n'
    exit 1
  fi
fi

printf 'Making it executable\n'
chmod +x "${lapce_dir}/devforge"

printf 'devforge-proxy installed\n'

exit 0
