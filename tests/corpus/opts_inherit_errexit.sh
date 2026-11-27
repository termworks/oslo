# mode: bash
# `inherit_errexit` makes a command substitution keep `set -e`.
set -e
shopt -s inherit_errexit
echo "[$(false; echo not-printed)]"
echo "$(echo $-)"
x=$(false; echo inner)
echo unreachable
