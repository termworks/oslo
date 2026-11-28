# mode: bash
# A directory or program named with a byte that is not UTF-8 is reached by that byte: `cd`,
# `pwd`, `$PWD`, a glob through it, and a path command. xtrace quotes the byte as bash does.
mkdir $'d\xff'
touch $'d\xff/f'
echo d*/* | od -An -tx1
cd $'d\xff'
p=$(pwd)
printf %s "${p##*/}" | od -An -tx1
printf %s "${PWD##*/}" | od -An -tx1
p=$(pwd -P)
printf %s "${p##*/}" | od -An -tx1
cd ..
printf %s "${OLDPWD##*/}" | od -An -tx1
printf '#!/bin/sh\necho ran\n' > $'s\xfe'
chmod +x $'s\xfe'
./$'s\xfe'
x=$'\xff'
{ set -x; : "$x"; set +x; } 2>&1 | od -An -tx1
select s in "$x"; do break; done <<< 1 2>&1 | od -An -tx1
