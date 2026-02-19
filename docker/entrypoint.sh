#!/bin/sh
set -eu

APP_DIR="${APP_DIR:-/app}"
DATA_DIR="${DATA_DIR:-${APP_DIR}/data}"
DEFAULT_CONFIG="${APP_DIR}/config.defaults.toml"
CONFIG_PATH="${DATA_DIR}/config.toml"
TOKEN_PATH="${DATA_DIR}/token.json"

# 以 root 身份修复挂载卷权限
mkdir -p "${DATA_DIR}"
chown -R appuser:appuser "${DATA_DIR}"

if [ ! -f "${CONFIG_PATH}" ] && [ -f "${DEFAULT_CONFIG}" ]; then
  cp "${DEFAULT_CONFIG}" "${CONFIG_PATH}"
  echo "[entrypoint] data/config.toml not found, created from config.defaults.toml"
fi

if [ ! -f "${TOKEN_PATH}" ]; then
  printf '{\n  "ssoBasic": []\n}\n' > "${TOKEN_PATH}"
  echo "[entrypoint] data/token.json not found, created empty ssoBasic pool"
fi

# 降权到 appuser 执行主进程
exec gosu appuser "$@"
