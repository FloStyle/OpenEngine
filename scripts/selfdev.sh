#!/usr/bin/env bash
# OpenEngine self-development loop (canned observe→propose→verify→apply).
# Usage: bash scripts/selfdev.sh [url]
set -euo pipefail
URL="${1:-http://127.0.0.1:8080}"
B=$URL

echo "== observe =="; curl -s "$B/observe"
echo; echo "== snapshot (pre-change state) =="
SNAP=$(curl -s "$B/snapshot")
echo "$SNAP" | python3 -c 'import sys,json;d=json.load(sys.stdin);print("entities",len(d.get("entities",[])),"tick",d.get("tick"))'
echo "== propose: spawn an entity =="
curl -s -X POST "$B/spawn" -H 'Content-Type: application/json' \
  -d '{"transform":[1,0,0],"color":[255,0,0,255]}'
echo; echo "== verify repo gates =="
curl -s "$B/verify" | python3 -c 'import sys,json;d=json.load(sys.stdin);print("verify",d.get("status"),"errors",d.get("errors"))'
echo "== determinism prove =="
curl -s -X POST "$B/prove" -H 'Content-Type: application/json' -d '{"n":50}' | python3 -c 'import sys,json;d=json.load(sys.stdin);print("equal",d.get("equal"))'
echo "== roll back to the snapshot (reversible apply) =="
python3 -c "import json;json.dump({'snapshot':json.loads('''$SNAP''')}, open('/tmp/oe_restore.json','w'))"
curl -s -X POST "$B/restore" -H 'Content-Type: application/json' -d @/tmp/oe_restore.json
echo
echo "== observe (should be back to pre-change) =="; curl -s "$B/observe" | python3 -c 'import sys,json;d=json.load(sys.stdin);print("entities",d["entity_count"])'
