#!/usr/bin/env bash
# Generates node configs + genesis.toml for the 5-node docker-compose test network.
# Uses bcc-client node init to create each config with a fresh keypair.
#
# Usage: ./scripts/gen-test-configs.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
CONFIG_DIR="$ROOT_DIR/config"
BIN="$ROOT_DIR/target/release/bcc-client"

# Build bcc-client if needed.
if [ ! -x "$BIN" ]; then
    echo "Building bcc-client..."
    cargo build --release -p bcc-client --manifest-path "$ROOT_DIR/Cargo.toml"
fi

mkdir -p "$CONFIG_DIR"

# Docker-compose assigns 172.30.0.2–172.30.0.6 to node1–node5.
IPS=(172.30.0.2 172.30.0.3 172.30.0.4 172.30.0.5 172.30.0.6)
N=${#IPS[@]}

declare -a ADDRESSES
declare -a PUBKEYS

echo "Generating configs for $N nodes..."
echo ""

for i in $(seq 1 "$N"); do
    IDX=$((i - 1))

    PEER_ARGS=()
    for j in $(seq 0 $((N - 1))); do
        if [ "$j" -ne "$IDX" ]; then
            PEER_ARGS+=(--peer "${IPS[$j]}:8333")
        fi
    done

    OUT=$("$BIN" node init \
        --output "$CONFIG_DIR/node${i}.toml" \
        --sled-path "/data/node${i}" \
        --genesis-path "/app/config/genesis.toml" \
        "${PEER_ARGS[@]}" 2>&1)

    echo "node${i}: $OUT"

    ADDRESSES[$IDX]=$(echo "$OUT" | grep '^Address:' | awk '{print $2}')
    PUBKEYS[$IDX]=$(echo "$OUT"   | grep '^Pubkey:'  | awk '{print $2}')
done

# Write genesis.toml from the freshly generated addresses/pubkeys.
GENESIS="$CONFIG_DIR/genesis.toml"
TIMESTAMP=$(date +%s)

{
    echo "timestamp = $TIMESTAMP"
    echo ""
    for i in $(seq 0 $((N - 1))); do
        echo "[[validators]]"
        echo "address = \"${ADDRESSES[$i]}\""
        echo "pubkey  = \"${PUBKEYS[$i]}\""
        echo "stake   = 1000000000000"
        echo ""
    done
} > "$GENESIS"

echo ""
echo "Done."
echo "  Configs : $CONFIG_DIR/node{1..${N}}.toml"
echo "  Genesis : $GENESIS"
