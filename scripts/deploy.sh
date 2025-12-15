#!/usr/bin/env bash
set -euo pipefail

# Rain 部署脚本：构建后端/前端、同步静态资源、配置 systemd 与 nginx。
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ENV_FILE="${ENV_FILE:-$ROOT/backend/.env}"
FRONTEND_BUILD="$ROOT/frontend/dist"
STATIC_DEST="${STATIC_DEST:-/var/www/rain}"
ORIG_USER="${SUDO_USER:-$(id -un)}"
ORIG_HOME="$(getent passwd "$ORIG_USER" | cut -d: -f6)"

ensure_root() {
  if [[ ${EUID:-$(id -u)} -ne 0 ]]; then
    echo "This script must be run as root (use sudo)." >&2
    exit 1
  fi
}

ensure_root

load_env_file() {
  if [[ -f "$ENV_FILE" ]]; then
    # shellcheck disable=SC1090
    set -a && source "$ENV_FILE" && set +a
  fi
}

load_env_file

SERVICE_USER="${SERVICE_USER:-root}"
SERVICE_GROUP="${SERVICE_GROUP:-$SERVICE_USER}"
SERVICES=("rain-backend")
BACKEND_UNIT_PATH="/etc/systemd/system/rain-backend.service"

usage() {
  echo "Usage: $0 [install|start|stop|restart|status|build|clean-static]" >&2
  exit 1
}

ACTION="${1:-start}"
shift || true
NGINX_SERVICE="${NGINX_SERVICE:-nginx}"

build() {
  if [[ -n "$ORIG_HOME" ]]; then
    export HOME="$ORIG_HOME"
    if [[ -f "$ORIG_HOME/.cargo/env" ]]; then
      # shellcheck disable=SC1090
      source "$ORIG_HOME/.cargo/env"
    fi
    if [[ -s "$ORIG_HOME/.nvm/nvm.sh" ]]; then
      export NVM_DIR="${NVM_DIR:-$ORIG_HOME/.nvm}"
      # shellcheck disable=SC1090
      source "$ORIG_HOME/.nvm/nvm.sh"
    fi
  fi
  bash "$ROOT/scripts/build.sh"
}

sync_static_assets() {
  STATIC_ROOT="$STATIC_DEST"
  if [[ ! -d "$FRONTEND_BUILD" ]]; then
    echo "frontend build not found at $FRONTEND_BUILD; run build first" >&2
    exit 1
  fi
  mkdir -p "$STATIC_ROOT"
  rsync -a --delete "$FRONTEND_BUILD"/ "$STATIC_ROOT"/
}

clean_static() {
  [[ -d "$STATIC_DEST" ]] || { echo "No static dir at $STATIC_DEST"; return; }
  rm -rf "$STATIC_DEST"
  echo "Removed static assets at $STATIC_DEST"
}

read_nginx_vars() {
  DOMAIN="${DOMAIN:-${DEPLOY_DOMAIN:-${RAIN_DEPLOY_DOMAIN:-}}}"
  EXTERNAL_PORT="${EXTERNAL_PORT:-${DEPLOY_EXTERNAL_PORT:-${RAIN_DEPLOY_EXTERNAL_PORT:-443}}}"
  CERT_PATH="${CERT_PATH:-${DEPLOY_CERT_PATH:-${RAIN_SSL_CERT_PATH:-}}}"
  KEY_PATH="${KEY_PATH:-${DEPLOY_KEY_PATH:-${RAIN_SSL_KEY_PATH:-}}}"
  BACKEND_BIND="${BACKEND_BIND:-${DEPLOY_BACKEND_BIND:-${RAIN_BACKEND_BIND:-}}}"

  if [[ -z "$BACKEND_BIND" ]]; then
    host="${SERVER_HOST:-127.0.0.1}"
    port="${SERVER_PORT:-8080}"
    BACKEND_BIND="${host}:${port}"
  fi

  if [[ -z "${DOMAIN:-}" || -z "${CERT_PATH:-}" || -z "${KEY_PATH:-}" ]]; then
    cat >&2 <<EOF
nginx requires DOMAIN, CERT_PATH, KEY_PATH.
Provide them via environment variables or $ENV_FILE, e.g.:
  DOMAIN=rain.example.com
  CERT_PATH=/etc/letsencrypt/live/rain/fullchain.pem
  KEY_PATH=/etc/letsencrypt/live/rain/privkey.pem
EOF
    exit 1
  fi
}

configure_nginx() {
  read_nginx_vars
  sync_static_assets

  local nginx_conf="/etc/nginx/sites-available/rain.conf"
  cat >"$nginx_conf" <<EOF
server {
    listen 80;
    server_name $DOMAIN;
    return 301 https://\$host:$EXTERNAL_PORT\$request_uri;
}

server {
    listen $EXTERNAL_PORT ssl;
    server_name $DOMAIN;

    ssl_certificate $CERT_PATH;
    ssl_certificate_key $KEY_PATH;

    root $STATIC_ROOT;
    index index.html;

    location /api/ {
        proxy_pass http://$BACKEND_BIND;
        proxy_set_header Host \$host;
        proxy_set_header X-Real-IP \$remote_addr;
        proxy_set_header X-Forwarded-For \$proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto \$scheme;
        proxy_http_version 1.1;
    }

    location /ws/ {
        proxy_pass http://$BACKEND_BIND;
        proxy_set_header Host \$host;
        proxy_set_header X-Real-IP \$remote_addr;
        proxy_set_header X-Forwarded-For \$proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto \$scheme;
        proxy_http_version 1.1;
        proxy_set_header Upgrade \$http_upgrade;
        proxy_set_header Connection "upgrade";
    }

    location / {
        try_files \$uri /index.html;
    }
}
EOF

  ln -sf "$nginx_conf" /etc/nginx/sites-enabled/rain.conf
}

write_unit_files() {
  tee "$BACKEND_UNIT_PATH" >/dev/null <<EOF
[Unit]
Description=Rain Backend
After=network-online.target
Wants=network-online.target

[Service]
WorkingDirectory=$ROOT/backend
ExecStart=$ROOT/backend/target/release/backend
Restart=on-failure
RestartSec=3
User=$SERVICE_USER
Group=$SERVICE_GROUP

[Install]
WantedBy=multi-user.target
EOF
}

start_services() {
  systemctl daemon-reload
  for svc in "${SERVICES[@]}"; do
    systemctl start "${svc}.service"
  done
}

stop_services() {
  for ((idx=${#SERVICES[@]}-1; idx>=0; idx--)); do
    systemctl stop "${SERVICES[idx]}.service" >/dev/null 2>&1 || true
  done
}

status_services() {
  for svc in "${SERVICES[@]}"; do
    systemctl status "${svc}.service" --no-pager
  done
}

reload_nginx() {
  nginx -t
  systemctl reload "${NGINX_SERVICE}.service"
}

case "$ACTION" in
  install)
    stop_services
    build
    write_unit_files
    configure_nginx
    start_services
    reload_nginx
    ;;
  build)
    build
    ;;
  start)
    build
    write_unit_files
    configure_nginx
    start_services
    reload_nginx
    ;;
  stop)
    stop_services
    ;;
  restart)
    stop_services
    build
    write_unit_files
    configure_nginx
    start_services
    reload_nginx
    ;;
  status)
    status_services
    ;;
  clean-static)
    clean_static
    ;;
  *)
    usage
    ;;
esac
