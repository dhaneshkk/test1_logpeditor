#!/bin/sh
# Check the build requirements of the IOGP stack: Rust >= 1.98 (pinned in rust-toolchain.toml),
# CMake >= 3.16 and a C/C++ compiler (clingox builds clingo from source), git (Scryer fork).
ok=1
need() { printf '%s\n%s\n' "$2" "$1" | sort -V | head -n1 | grep -qx "$2"; }
rv=$(rustc --version 2>/dev/null | awk '{print $2}')
if [ -z "$rv" ]; then echo "✗ Rust not found — install rustup: curl -sSf https://sh.rustup.rs | sh"; ok=0
elif need "$rv" 1.98.0; then echo "✓ Rust $rv"; else echo "✗ Rust $rv < 1.98 — run: rustup toolchain install 1.98 (rust-toolchain.toml selects it)"; ok=0; fi
cv=$(cmake --version 2>/dev/null | awk 'NR==1{print $3}')
if [ -z "$cv" ]; then echo "✗ CMake not found — Ubuntu/WSL: sudo apt install cmake"; ok=0
elif need "$cv" 3.16; then echo "✓ CMake $cv"; else echo "✗ CMake $cv < 3.16 — upgrade CMake"; ok=0; fi
if command -v c++ >/dev/null 2>&1 || command -v g++ >/dev/null 2>&1 || command -v clang++ >/dev/null 2>&1; then echo "✓ C++ compiler"; else echo "✗ no C++ compiler — Ubuntu/WSL: sudo apt install build-essential"; ok=0; fi
command -v git >/dev/null 2>&1 && echo "✓ git" || { echo "✗ git not found — sudo apt install git"; ok=0; }
[ $ok = 1 ] && echo "all build requirements met" || { echo "missing build requirements (see above)"; exit 1; }
