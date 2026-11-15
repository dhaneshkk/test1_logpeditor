#!/bin/sh
# Build the C demo against the static library and run it with a config.json.
set -e
cd "$(dirname "$0")"
T=${CARGO_TARGET_DIR:-../../target}
(cd ../.. && cargo build --release -q)
cc -O2 -Wall -Wextra demo.c -I../../include "$T/release/libiogp_runtime.a" -lpthread -ldl -lm -o demo
./demo "${1:-../../tests/fixtures/config.json}"
