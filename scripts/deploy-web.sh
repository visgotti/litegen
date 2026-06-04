#!/usr/bin/env bash
# Build the LiteGen dashboard and serve it on the droplet's :80 (nginx) in front
# of the already-running API (:4000). Same-origin so session cookies/CSRF work.
#
#   ./scripts/deploy-web.sh
#
# Reads DO_DROPLET_IP + DO_SSH_KEY_PATH from .env.deploy.
set -euo pipefail
cd "$(dirname "$0")/.."   # repo root

set -a; . ./.env.deploy; set +a
KEY="${DO_SSH_KEY_PATH/#\~/$HOME}"
IP="${DO_DROPLET_IP:?DO_DROPLET_IP not set}"
# Public origin of the app (Cloudflare-fronted). Override for testing, e.g.
#   APP_URL=http://$IP ./scripts/deploy-web.sh
APP_URL="${APP_URL:-https://app.litegen.ai}"
API_BASE="${APP_URL}/api"   # API is served under /api (nginx strips the prefix)
SSH="ssh -i ${KEY} -o StrictHostKeyChecking=accept-new -o ConnectTimeout=20 root@${IP}"

echo "==> Building dashboard (VITE_API_URL=${API_BASE})"
( cd dashboard && VITE_API_URL="${API_BASE}" npm run build )

echo "==> Packaging dist"
tar -C dashboard -czf /tmp/litegen-dashboard-dist.tgz dist

echo "==> Shipping dist + nginx.conf + web compose to droplet"
scp -i "${KEY}" -o StrictHostKeyChecking=accept-new \
  /tmp/litegen-dashboard-dist.tgz \
  deploy/nginx.conf \
  deploy/docker-compose.web.yml \
  root@"${IP}":/opt/litegen/

echo "==> Unpacking + bringing up web tier on droplet"
$SSH '
  set -e
  cd /opt/litegen
  rm -rf dashboard-dist && mkdir dashboard-dist
  tar -C dashboard-dist --strip-components=1 -xzf litegen-dashboard-dist.tgz   # dist/* -> dashboard-dist/*
  docker compose -f docker-compose.prod.yml -f docker-compose.web.yml --env-file .env up -d
  echo "--- compose ps ---"
  docker compose -f docker-compose.prod.yml -f docker-compose.web.yml ps
'
echo "==> Done. Dashboard live at ${APP_URL}/ (API at ${API_BASE})"
