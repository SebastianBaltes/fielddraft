#!/usr/bin/env bash
# FieldDraft platform smoke test — exercises the full User→Agent happy path
# against a running server + real MariaDB. Run this once before going live and
# after a deploy. It is re-runnable (each run uses a fresh random user email).
#
# Requires: curl, jq, and a running server with a seeded admin + the two agent
# service tokens. Configure via env (defaults match the local dev run):
#   FD_API                 base URL            (default http://127.0.0.1:8080)
#   ADMIN_EMAIL/PASSWORD   seeded admin login  (default admin@fd.test / adminpass123)
#   PO_SERVICE_TOKEN       PO agent token      (default po-tok-xyz)
#   CODING_SERVICE_TOKEN   coding agent token  (default coding-tok-xyz)
#
# Exit code = number of failed assertions (0 = all good).
set -u
API=${FD_API:-http://127.0.0.1:8080}
ADMIN_EMAIL=${ADMIN_EMAIL:-admin@fd.test}
ADMIN_PASSWORD=${ADMIN_PASSWORD:-adminpass123}
PO=${PO_SERVICE_TOKEN:-po-tok-xyz}
CODING=${CODING_SERVICE_TOKEN:-coding-tok-xyz}
USER_EMAIL="smoke-$RANDOM$RANDOM@fd.test"

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
cd "$WORK"
PASS=0; FAIL=0
chk(){ if [ "$2" = "$3" ]; then echo "[PASS] $1 ($2)"; PASS=$((PASS+1));
       else echo "[FAIL] $1 — got '$2' want '$3'"; FAIL=$((FAIL+1)); fi; }
J(){ jq -r "$1" "$2" 2>/dev/null; }

echo "── 1. admin login ──"
code=$(curl -s -c admin.jar -o admin.json -w '%{http_code}' -X POST "$API/api/auth/login" \
  -H 'Content-Type: application/json' -d "{\"email\":\"$ADMIN_EMAIL\",\"password\":\"$ADMIN_PASSWORD\"}")
chk "admin login http" "$code" "200"
chk "admin role" "$(J .role admin.json)" "admin"

echo "── 2. admin creates an invite ──"
code=$(curl -s -b admin.jar -o invite.json -w '%{http_code}' -X POST "$API/api/invites" \
  -H 'Content-Type: application/json' -d '{"note":"smoke"}')
chk "invite create http (201 Created)" "$code" "201"
TOKEN=$(J .token invite.json)
[ -n "$TOKEN" ] && [ "$TOKEN" != "null" ] && { echo "[PASS] invite token present"; PASS=$((PASS+1)); } || { echo "[FAIL] no invite token"; FAIL=$((FAIL+1)); }

echo "── 3. registration is invite-gated ──"
code=$(curl -s -o /dev/null -w '%{http_code}' -X POST "$API/api/auth/register" \
  -H 'Content-Type: application/json' -d "{\"email\":\"$USER_EMAIL\",\"display_name\":\"Smoke\",\"password\":\"userpass123\"}")
chk "register without invite rejected (4xx)" "$([ "$code" -ge 400 ] && [ "$code" -lt 500 ] && echo yes)" "yes"
code=$(curl -s -c user.jar -o reg.json -w '%{http_code}' -X POST "$API/api/auth/register" \
  -H 'Content-Type: application/json' -d "{\"email\":\"$USER_EMAIL\",\"display_name\":\"Smoke\",\"password\":\"userpass123\",\"invite\":\"$TOKEN\"}")
chk "register with invite http" "$code" "200"
chk "new user role" "$(J .role reg.json)" "user"
code=$(curl -s -o /dev/null -w '%{http_code}' -X POST "$API/api/auth/register" \
  -H 'Content-Type: application/json' -d "{\"email\":\"other-$USER_EMAIL\",\"display_name\":\"X\",\"password\":\"userpass123\",\"invite\":\"$TOKEN\"}")
chk "invite is single-use (4xx on reuse)" "$([ "$code" -ge 400 ] && [ "$code" -lt 500 ] && echo yes)" "yes"

echo "── 4. project CRUD ──"
code=$(curl -s -b user.jar -o proj.json -w '%{http_code}' -X POST "$API/api/projects" \
  -H 'Content-Type: application/json' -d '{"name":"Smoke Project","document":{"v":2,"parts":[]}}')
chk "project create http (201)" "$code" "201"
chk "project default visibility" "$(J .visibility proj.json)" "private"
chk "project list count" "$(curl -s -b user.jar "$API/api/projects" | jq 'length')" "1"

echo "── 5. user files a ticket ──"
code=$(curl -s -b user.jar -o tkt.json -w '%{http_code}' -X POST "$API/api/tickets" \
  -H 'Content-Type: application/json' -d '{"title":"Smoke bug","body":"Something is off.","category":"bug"}')
chk "ticket create http (201)" "$code" "201"
TID=$(J .id tkt.json)
chk "ticket initial state" "$(J .state tkt.json)" "triage"

echo "── 6. agent auth (negatives) ──"
chk "agent queue no token → 401" "$(curl -s -o /dev/null -w '%{http_code}' "$API/api/agent/queue")" "401"
chk "agent queue bad token → 401" "$(curl -s -o /dev/null -w '%{http_code}' -H 'Authorization: Bearer nope' "$API/api/agent/queue")" "401"

echo "── 7. PO triages the queue + accepts (creates work-order) ──"
qhit=$(curl -s -H "Authorization: Bearer $PO" "$API/api/agent/queue" | jq "[.items[]?|select(.id==\"$TID\")]|length")
chk "ticket appears in PO triage queue" "$qhit" "1"
VERDICT="{\"spam\":false,\"malicious\":false,\"vision_fit\":\"fit\",\"confidence\":0.9,\"category\":\"bug\",\"reason\":\"reproducible\",\"public_reply\":\"Thanks, reproduced — queuing a fix.\",\"next_state\":\"accepted\",\"work_order\":{\"type\":\"bug\",\"summary\":\"fix the thing\",\"acceptance_criteria\":[\"it works\"],\"vision_notes\":\"core\"}}"
chk "verdict accept http" "$(curl -s -o /dev/null -w '%{http_code}' -H "Authorization: Bearer $PO" -X POST "$API/api/tickets/$TID/verdict" -H 'Content-Type: application/json' -d "$VERDICT")" "200"
chk "state after accept" "$(curl -s -b user.jar "$API/api/tickets/$TID" | jq -r .state)" "accepted"

echo "── 8. coding: claim → deploying → resolve ──"
chk "coding sees work-order in queue" "$(curl -s -H "Authorization: Bearer $CODING" "$API/api/agent/queue" | jq "[.items[]?|select(.id==\"$TID\")]|length")" "1"
chk "work-order claim http" "$(curl -s -o /dev/null -w '%{http_code}' -H "Authorization: Bearer $CODING" -X POST "$API/api/work-orders/$TID/claim")" "200"
chk "state after claim" "$(curl -s -b user.jar "$API/api/tickets/$TID" | jq -r .state)" "in_progress"
code=$(curl -s -o deploy.json -w '%{http_code}' -H "Authorization: Bearer $CODING" -X POST "$API/api/deploys" \
  -H 'Content-Type: application/json' -d "{\"version\":\"0.0.0\",\"git_sha\":\"smoke\",\"ticket_ids\":[\"$TID\"],\"status\":\"live\"}")
chk "deploy record http (201)" "$code" "201"
DID=$(J .id deploy.json)
chk "transition to deploying" "$(curl -s -o /dev/null -w '%{http_code}' -H "Authorization: Bearer $CODING" -X PATCH "$API/api/tickets/$TID/agent-state" -H 'Content-Type: application/json' -d '{"state":"deploying"}')" "200"
chk "work-order complete http" "$(curl -s -o /dev/null -w '%{http_code}' -H "Authorization: Bearer $CODING" -X POST "$API/api/work-orders/$TID/complete" -H 'Content-Type: application/json' -d "{\"deploy_id\":\"$DID\"}")" "200"
chk "state after complete" "$(curl -s -b user.jar "$API/api/tickets/$TID" | jq -r .state)" "resolved"

echo "── 9. scope isolation: coding token cannot write a verdict ──"
chk "coding token on verdict → 403" "$(curl -s -o /dev/null -w '%{http_code}' -H "Authorization: Bearer $CODING" -X POST "$API/api/tickets/$TID/verdict" -H 'Content-Type: application/json' -d "$VERDICT")" "403"

echo "── 10. leak filter on outgoing PO comments ──"
curl -s -o /dev/null -H "Authorization: Bearer $PO" -X POST "$API/api/tickets/$TID/agent-comment" \
  -H 'Content-Type: application/json' -d '{"kind":"po","body":"Patch:\n```go\nfunc fix(){}\n```\nin server/main.go"}'
curl -s -o /dev/null -H "Authorization: Bearer $PO" -X POST "$API/api/tickets/$TID/agent-comment" \
  -H 'Content-Type: application/json' -d '{"kind":"po","body":"All clear, the fix is live."}'
chk "clean PO comment visible to user" "$(curl -s -b user.jar "$API/api/tickets/$TID" | jq '[.comments[]|select(.body|test("All clear"))]|length')" "1"
chk "leaky PO comment hidden from user" "$(curl -s -b user.jar "$API/api/tickets/$TID" | jq '[.comments[]|select(.body|test("func fix"))]|length')" "0"
chk "leaky PO comment visible to admin" "$(curl -s -b admin.jar "$API/api/tickets/$TID" | jq '[.comments[]|select(.body|test("func fix"))]|length')" "1"

echo ""
echo "════════ SMOKE TEST: $PASS passed, $FAIL failed ════════"
exit $FAIL
