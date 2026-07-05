#!/bin/zsh
export PORT="8787"

if lsof -iTCP:8787 -sTCP:LISTEN >/dev/null 2>&1; then
  echo "MiniMax proxy already running on port 8787"
  sleep 999999
  exit 0
fi

cd "$HOME/.config/zed/ai_proxy"
exec "$HOME/.bun/bin/bun" run server.ts
