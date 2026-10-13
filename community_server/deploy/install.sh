#!/bin/sh
# Dedicated API only. Existing websites, databases and certificate renewal are untouched.
set -eu
test "$(id -u)" = 0
test ! -e /opt/clipforge-ingest || test -f /opt/clipforge-ingest/.clipforge-install-incomplete
test ! -e /var/lib/clipforge-ingest
test -f /root/clipforge-ingest-source.tar.gz
test -f /opt/credenta-review/acme/live/credenta-ip/fullchain.pem
test "$(df --output=avail -B1 / | tail -1)" -gt 75161927680
! ss -lnt | grep -Eq ':(4173|9443)[[:space:]]'
mkdir -p /root/clipforge-deployment
find /etc/nginx -type f -print0 | sort -z | xargs -0 sha256sum > /root/clipforge-deployment/nginx-before.sha256
curl -fsS -o /dev/null https://exactpic.com
curl -fsS -o /dev/null https://157.173.114.38:8443
opened=0
configured=0
cleanup() {
  if [ "$configured" = 1 ]; then
    if [ "$opened" = 1 ]; then ufw delete allow 9443/tcp || true; fi
    rm -f /etc/nginx/conf.d/clipforge-ingest.conf
    nginx -t && systemctl reload nginx || true
    systemctl stop clipforge-ingest.service clipforge-maintain.timer || true
  fi
}
trap cleanup HUP INT TERM EXIT
id clipforge-ingest >/dev/null 2>&1 || useradd --system --home-dir /var/lib/clipforge-ingest --shell /usr/sbin/nologin clipforge-ingest
install -d -m 0755 /opt/clipforge-ingest
touch /opt/clipforge-ingest/.clipforge-install-incomplete
tar -xzf /root/clipforge-ingest-source.tar.gz -C /opt/clipforge-ingest --strip-components=1
python3 -m venv --without-pip /opt/clipforge-ingest/venv
/opt/clipforge-ingest/venv/bin/python /opt/clipforge-ingest/deploy/bootstrap.py
install -d -m 0700 -o clipforge-ingest -g clipforge-ingest /var/lib/clipforge-ingest
install -d -m 0700 /etc/clipforge-ingest
python3 -c 'import pathlib,secrets; p=pathlib.Path("/etc/clipforge-ingest/token-pepper");p.write_text(secrets.token_hex(32));p.chmod(0o600)'
install -m 0644 /opt/clipforge-ingest/deploy/clipforge-ingest.service /opt/clipforge-ingest/deploy/clipforge-ingest.slice /opt/clipforge-ingest/deploy/clipforge-maintain.service /opt/clipforge-ingest/deploy/clipforge-maintain.timer /etc/systemd/system/
systemd-analyze verify /etc/systemd/system/clipforge-ingest.service /etc/systemd/system/clipforge-ingest.slice /etc/systemd/system/clipforge-maintain.service /etc/systemd/system/clipforge-maintain.timer
systemctl daemon-reload
configured=1
systemctl start clipforge-ingest.service
for count in 1 2 3 4 5; do curl -fsS http://127.0.0.1:4173/v1/health >/dev/null && break || sleep 1; done
cd /opt/clipforge-ingest
CLIPFORGE_TEST_ROOT=/root/clipforge-deployment/tests PYTHONDONTWRITEBYTECODE=1 /opt/clipforge-ingest/venv/bin/python -m unittest discover -s tests -q
/opt/clipforge-ingest/venv/bin/python deploy/smoke.py
install -m 0644 deploy/clipforge-ingest.conf /etc/nginx/conf.d/clipforge-ingest.conf
nginx -t
systemctl reload nginx
ufw allow 9443/tcp
opened=1
CLIPFORGE_SMOKE_ORIGIN=https://157.173.114.38:9443 /opt/clipforge-ingest/venv/bin/python deploy/smoke.py
sha256sum -c /root/clipforge-deployment/nginx-before.sha256 >/dev/null
curl -fsS -o /dev/null -w 'ExactPic after: %{http_code} %{time_total}\n' https://exactpic.com
curl -fsS -o /dev/null -w 'Credenta after: %{http_code} %{time_total}\n' https://157.173.114.38:8443
systemctl enable clipforge-ingest.service clipforge-maintain.timer
systemctl start clipforge-maintain.timer
systemctl show clipforge-ingest.slice -p MemoryMax -p CPUQuotaPerSecUSec -p TasksMax
systemctl is-active clipforge-ingest.service clipforge-maintain.timer credenta-university-api.service credenta-university-web.service nginx.service
trap - HUP INT TERM EXIT
rm -f /opt/clipforge-ingest/.clipforge-install-incomplete
printf 'Dedicated receiver deployment passed. Existing nginx files unchanged.\n'
