#!/usr/bin/env bash
# run_oracle.sh <dataDir> <wiki> <profile> <apeURL> <out.jsonl> [parser]
set -euo pipefail
J=${ACEWIKI_JAVA:-/home/claude/acewiki-java}; L=${ACEWIKI_LIB:-/home/claude/src/acewiki/lib}
exec env JAVA_TOOL_OPTIONS= java -Xss64m -Xmx3g -cp "$J/shim:$J/classes:$L/*" Oracle "$@"
