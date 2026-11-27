# mode: bash
self() { local line; while read -r line; do echo "${line//"$0"/SELF}"; done; }
caller; echo "top st=$?"
caller 0; echo "top st=$?"
echo "[${BASH_SOURCE[*]}] [${BASH_LINENO[*]}] [${FUNCNAME[*]}] ${#BASH_SOURCE[@]}"
f() {
  echo "f: ${#BASH_SOURCE[@]} ${BASH_LINENO[*]} ${FUNCNAME[*]} $LINENO"
  caller | self; echo "st=${PIPESTATUS[0]}"
  caller 0 | self; echo "st=${PIPESTATUS[0]}"
  caller 1; echo "st=$?"
  caller x 2>/dev/null; echo "st=$?"
}
g() { f; }
g
printf '%s\n' 'echo "lib: ${BASH_SOURCE[0]} ${BASH_LINENO[*]} [${FUNCNAME[*]}]"' 'caller | self' > lib.sh
source ./lib.sh
s() { source ./lib.sh; }
s
