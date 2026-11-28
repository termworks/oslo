# mode: bash
# Variables bash defines before the first command: `$_`, `HOSTNAME`, and an exported `OLDPWD`
# with no value when none was inherited.
declare -p _ | sed 's/=.*//'
declare -p HOSTNAME | sed 's/=.*//'
[ "$HOSTNAME" = "$(uname -n)" ] && echo "hostname ok"
declare -p OLDPWD
declare -x | grep -c '^declare -x OLDPWD$'
echo "${OLDPWD-unset}"
mkdir d && cd d && cd ..
declare -p OLDPWD | sed 's/=.*//'
