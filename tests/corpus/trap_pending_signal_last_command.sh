# mode: bash
# A trapped signal that arrives during the last command runs its handler before the shell ends.
trap 'echo "got USR1 $BASH_TRAPSIG"' USR1
echo "[${BASH_TRAPSIG-unset}]"
kill -USR1 $$
