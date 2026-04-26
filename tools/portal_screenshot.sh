#!/usr/bin/env bash
set -euo pipefail

# Usage: ./tools/portal_screenshot.sh [output_path]
# Default output: /tmp/portal_debug.png

OUTPUT="${1:-/tmp/portal_debug.png}"
BRP="http://localhost:15702"
PLAYER_ENTITY=""  # will be queried

# 1. Query portals
PORTALS=$(curl -sf -X POST "$BRP" -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"dm/query_portals","params":{},"id":1}')
echo "Portals: $PORTALS"

# 2. Get first portal position
PORTAL_X=$(echo "$PORTALS" | python3 -c "import sys,json; r=json.load(sys.stdin)['result'][0]; print(r['x'])")
PORTAL_Y=$(echo "$PORTALS" | python3 -c "import sys,json; r=json.load(sys.stdin)['result'][0]; print(r['y'])")
PORTAL_Z=$(echo "$PORTALS" | python3 -c "import sys,json; r=json.load(sys.stdin)['result'][0]; print(r.get('z',0))")

# 3. Query player entity
PLAYER=$(curl -sf -X POST "$BRP" -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"world.query","params":{"data":{"components":["fellytip_shared::components::LocalPlayer"]}},"id":2}')
PLAYER_ENTITY=$(echo "$PLAYER" | python3 -c "import sys,json; print(json.load(sys.stdin)['result'][0]['entity'])")

# 4. Teleport player near portal (offset by 3 units)
NEAR_X=$(python3 -c "print($PORTAL_X + 3)")
NEAR_Y=$(python3 -c "print($PORTAL_Y + 3)")
curl -sf -X POST "$BRP" -H "Content-Type: application/json" \
  -d "{\"jsonrpc\":\"2.0\",\"method\":\"dm/teleport\",\"params\":{\"entity\":$PLAYER_ENTITY,\"x\":$NEAR_X,\"y\":$NEAR_Y,\"z\":$PORTAL_Z},\"id\":3}"

# 5. Enable portal debug overlay
curl -sf -X POST "$BRP" -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"dm/set_portal_debug","params":{"enabled":true},"id":4}'

# 6. Wait for render to settle
sleep 2

# 7. Screenshot
scrot "$OUTPUT" --display :0
echo "Screenshot saved to $OUTPUT"
