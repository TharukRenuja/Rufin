#!/usr/bin/env bash
set -euo pipefail

cargo run --locked -p xtask -- release prepare "$@"
