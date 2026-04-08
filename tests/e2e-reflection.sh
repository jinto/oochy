#!/bin/bash
# E2E test: Reflection loop — repeated messages → skill suggestion
# Requires: real LLM API key configured, kittypaw binary built
# Usage: ./tests/e2e-reflection.sh
set -euo pipefail
cd "$(dirname "$0")/.."

BINARY="./target/debug/kittypaw"

echo "=== E2E: Reflection Loop ==="

# Step 0: Cleanup
echo ">>> Step 0: Cleanup"
$BINARY reflection clear 2>/dev/null || true

# Step 1: Send 3 similar messages
echo ""
echo ">>> Step 1: Send 3 similar messages"
for MSG in "환율 알려줘" "달러 가격 얼마야" "오늘 환율이 얼마야"; do
    $BINARY test-event "$MSG" --chat-id e2e-reflection >/dev/null 2>&1 || true
    echo "  Sent: $MSG"
done

# Step 2: Manually trigger reflection (no serve needed)
echo ""
echo ">>> Step 2: Run reflection"
$BINARY reflection run 2>&1

# Step 3: Check results
echo ""
echo ">>> Step 3: Check results"
$BINARY reflection list 2>&1

# Step 4: Test approve → skill creation
echo ""
echo ">>> Step 4: Test approve → skill creation"
LIST_OUTPUT=$($BINARY reflection list 2>&1)
HASH=$(echo "$LIST_OUTPUT" | grep "hash:" | head -1 | sed 's/.*hash: //' | tr -d '[:space:]')
if [ -n "$HASH" ]; then
    echo "  Approving hash: $HASH"
    $BINARY reflection approve "$HASH" 2>&1

    # Verify skill was created
    echo ""
    echo "  Checking skills list..."
    SKILLS=$($BINARY skills list 2>&1)
    echo "$SKILLS"
    if echo "$SKILLS" | grep -qi "환율\|달러\|exchange"; then
        echo "PASS: Skill created from reflection suggestion!"
    else
        echo "WARN: Skill may not have been created (check output above)"
    fi
else
    echo "  SKIP: No candidates to approve"
fi

# Step 5: Test reject flow (send more messages, re-run, then reject)
echo ""
echo ">>> Step 5: Test reject flow"
$BINARY reflection clear 2>/dev/null || true
for MSG in "날씨 알려줘" "오늘 날씨" "날씨 어때"; do
    $BINARY test-event "$MSG" --chat-id e2e-reflection >/dev/null 2>&1 || true
done
$BINARY reflection run 2>&1
LIST_OUTPUT=$($BINARY reflection list 2>&1)
HASH=$(echo "$LIST_OUTPUT" | grep "hash:" | head -1 | sed 's/.*hash: //' | tr -d '[:space:]')
if [ -n "$HASH" ]; then
    echo "  Rejecting hash: $HASH"
    $BINARY reflection reject "$HASH"
    echo "  Re-running reflection..."
    $BINARY reflection run 2>&1
    echo "  Verify rejection — should say 'No new patterns':"
    $BINARY reflection list 2>&1
else
    echo "  SKIP: No candidates to reject"
fi

# Cleanup
echo ""
echo ">>> Cleanup"
$BINARY reflection clear 2>/dev/null || true

echo ""
echo "=== E2E: Reflection Loop DONE ==="
