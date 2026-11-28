# mode: bash
# `set -Eeuo pipefail` turns every one of its options on, and `$-` reports them.
set -Eeuo pipefail
echo "$-"
trap 'echo "ERR $?"' ERR
f() { false; echo no; }
f
echo not reached
