# mode: bash
# The ERR handler idiom: with `-E` it sees the failing command, its caller's line and frames.
set -Eeuo pipefail
err() { echo "ERR at ${BASH_LINENO[0]} in ${FUNCNAME[1]:-main}: $BASH_COMMAND"; }
trap err ERR
f() {
  false
}
f
