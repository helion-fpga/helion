#!/bin/sh
# Fail if any listed file exceeds HELION_MAX_ARTIFACT_BYTES (default 100 MiB).
# Commercial FPGA CAD installs are tens of GB. Helion release assets must not.
#
#   packaging/check-artifact-size.sh FILE...
set -eu

MAX=${HELION_MAX_ARTIFACT_BYTES:-104857600}
[ $# -gt 0 ] || {
    echo "usage: packaging/check-artifact-size.sh FILE..." >&2
    exit 1
}

fail=0
for f in "$@"; do
    [ -f "$f" ] || {
        echo "missing $f" >&2
        exit 1
    }
    sz=$(wc -c < "$f" | tr -d ' ')
    printf 'artifact %s %s bytes (max %s)\n' "$(basename "$f")" "$sz" "$MAX"
    if [ "$sz" -gt "$MAX" ]; then
        echo "too large: $f ($sz > $MAX)" >&2
        fail=1
    fi
done
exit "$fail"
