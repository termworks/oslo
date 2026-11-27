# mode: bash
# `SHELLOPTS` and `BASHOPTS` follow the options, and neither can be assigned.
echo "$SHELLOPTS"
echo "$BASHOPTS"
set -o pipefail -u
shopt -s extglob nullglob
echo "$SHELLOPTS"
echo "$BASHOPTS"
set +u
( SHELLOPTS=x ) 2>/dev/null || echo "SHELLOPTS is readonly"
( BASHOPTS=x ) 2>/dev/null || echo "BASHOPTS is readonly"
