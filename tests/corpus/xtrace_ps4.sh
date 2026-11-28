# mode: bash
exec 2>&1
PS4='+${LINENO}: '; set -x
echo one
echo two
PS4='>> '; echo $(echo in)
V=z; PS4='[$V] '; echo hi
PS4='$(echo X) '; echo hi
PS4='+ ${FUNCNAME[0]:-main}:${LINENO}: '
f() {
  echo in
}
f
PS4=; echo noprefix
unset PS4; echo unset
