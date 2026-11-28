# mode: bash
# `set -o` and `set +o` list every option bash has, sorted by name.
set -o
set +o
set -Eeuo pipefail
set -o | grep -E '^(errexit|errtrace|nounset|pipefail) '
