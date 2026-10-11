#!/usr/bin/env sh
set -eu
cd "$(dirname "$0")/.."
umask 077
mkdir -p .secrets
if [ ! -f .secrets/server-key ]; then
    openssl rand -hex 32 > .secrets/server-key
fi
chmod 700 .secrets
# Compose mounts this file into a non-root container; the private parent directory protects it on the host.
chmod 444 .secrets/server-key
printf 'Ключ создан в .secrets/server-key. Введите его в веб-интерфейсе.\nЗапуск: docker compose up --build -d\nАдрес: http://localhost:8080\n'
