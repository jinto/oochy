#!/bin/bash
# E2E test: Skill create → schedule registration → list
# Requires: KITTYPAW_API_KEY set, kittypaw binary built
# Usage: ./tests/e2e-schedule.sh
set -euo pipefail
cd "$(dirname "$0")/.."

BINARY="./target/debug/kittypaw"

# 격리된 임시 환경 — 실제 ~/.kittypaw 데이터 보호
export KITTYPAW_HOME
KITTYPAW_HOME=$(mktemp -d)
SKILLS_DIR="$KITTYPAW_HOME/skills"
DB="$KITTYPAW_HOME/kittypaw.db"
unset KITTYPAW_DB_PATH  # KITTYPAW_HOME 격리가 우선되도록
trap 'rm -rf "$KITTYPAW_HOME"' EXIT

cat > "$KITTYPAW_HOME/kittypaw.toml" <<TOML
[llm]
provider = "claude"
api_key = ""

[sandbox]
timeout_secs = 30
memory_limit_mb = 64
TOML

mkdir -p "$SKILLS_DIR"

echo "=== E2E: Schedule Skill Lifecycle (isolated: $KITTYPAW_HOME) ==="

# Step 1: Create a scheduled skill
echo ""
echo ">>> Step 1: Create skill"
RESULT=$($BINARY test-event "AI 뉴스 10분마다 요약해줘" --chat-id e2e-test 2>&1)
echo "$RESULT"

if echo "$RESULT" | grep -qi "설정\|생성\|완료"; then
    echo "PASS: Skill creation response OK"
else
    echo "FAIL: Unexpected response"
    exit 1
fi

# Step 2: Verify skill file exists with cron
echo ""
echo ">>> Step 2: Verify skill file"
TOML_COUNT=$(ls "$SKILLS_DIR"/*.toml 2>/dev/null | wc -l | tr -d ' ')
if [ "$TOML_COUNT" -gt 0 ]; then
    echo "PASS: $TOML_COUNT skill file(s) found"
else
    echo "FAIL: No skill files created"
    exit 1
fi

LATEST_TOML=$(ls -t "$SKILLS_DIR"/*.toml | head -1)
if grep -q "cron" "$LATEST_TOML"; then
    echo "PASS: cron field exists in $LATEST_TOML"
    grep "cron" "$LATEST_TOML"
else
    echo "FAIL: No cron field in $LATEST_TOML"
    cat "$LATEST_TOML"
    exit 1
fi

if grep -q 'type = "schedule"' "$LATEST_TOML"; then
    echo "PASS: trigger type is schedule"
else
    echo "FAIL: trigger type is not schedule"
    exit 1
fi

# Step 3: Skill.list returns the skill
echo ""
echo ">>> Step 3: Skill list"
LIST_RESULT=$($BINARY test-event "등록된 스킬 목록 보여줘" --chat-id e2e-test 2>&1)
echo "$LIST_RESULT"

if echo "$LIST_RESULT" | grep -qi "schedule\|스킬\|스케줄\|정기"; then
    echo "PASS: Skill list shows scheduled skill"
else
    echo "FAIL: Skill not found in list"
    exit 1
fi

echo ""
echo "=== ALL E2E TESTS PASSED ==="
