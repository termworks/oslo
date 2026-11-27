# mode: bash
cd /tmp || exit 1
pushd /usr >/dev/null; pushd /etc >/dev/null
declare -p DIRSTACK
DIRSTACK[1]=/var; dirs
DIRSTACK[0]=/; dirs; pwd
DIRSTACK[5]=/x; dirs
DIRSTACK=(a /usr); dirs
unset 'DIRSTACK[1]'; dirs
popd >/dev/null; echo "${DIRSTACK[@]}"
cd /; echo "${DIRSTACK[0]} ${#DIRSTACK[@]}"
