# mode: bash
echo "$BASH_VERSION"
declare -p BASH_VERSINFO
echo "$MACHTYPE $HOSTTYPE $OSTYPE"
(( BASH_VERSINFO[0] >= 5 )) && echo five
[[ $BASH_VERSION == 5.* ]] && echo prefix
