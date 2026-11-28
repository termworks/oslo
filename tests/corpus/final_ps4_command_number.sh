# mode: bash
# `\#` is the number of top-level commands read from a script; a `-c` string reads none.
exec 2>&1
PS4='+[\#] '
set -x
true
true; true
set +x
p='\#'
echo "${p@P}"
eval 'echo "${p@P}"'
