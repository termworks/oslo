# mode: bash
# `BASH_TRAPSIG` names the condition a handler runs for, and is unset outside handlers.
trap 'echo "D $BASH_TRAPSIG"' DEBUG
trap 'echo "R $BASH_TRAPSIG"' RETURN
f() { :; }
. /dev/null
trap - DEBUG RETURN
trap 'echo "ERR $BASH_TRAPSIG"' ERR
trap 'echo "EXIT $BASH_TRAPSIG"' EXIT
false
echo "[${BASH_TRAPSIG-unset}]"
