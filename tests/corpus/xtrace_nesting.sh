# mode: bash
exec 2>&1
set -x
echo $(echo inner)
x=$(echo $(echo deep))
eval 'echo e; eval "echo e2"'
(echo sub; (echo sub2))
f() { echo in; }; f arg; x=$(f)
printf 'echo src\n' > s.sh; source ./s.sh
echo `echo bt`
trap 'echo t' EXIT
echo body
