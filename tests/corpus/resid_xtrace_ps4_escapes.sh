# mode: bash
exec 2>&1
x=hi
PS4='[\$ $x \\ \101] '
set -x
true
PS4='+\040'
true
PS4='\[\]$(echo sub)\n> '
true
