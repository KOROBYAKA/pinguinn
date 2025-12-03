#!/usr/bin/env bash
set -euox pipefail

# Make sure PATH includes your user bins (for this shell)
export PATH="$HOME/.local/share/solana/install/active_release/bin:$HOME/.cargo/bin:$PATH"

SOLANA_BIN="$(command -v solana)" || { echo "solana not found in PATH"; exit 127; }

sudo "$SOLANA_BIN" gossip > gossip.log
sudo ./pinguinn_util
