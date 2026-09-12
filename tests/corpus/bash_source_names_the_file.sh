# mode: bash
# `dirname "${BASH_SOURCE[0]}"` is how a bash script finds its own directory. Unset, the usual
# `cd "$(dirname "${BASH_SOURCE[0]}")" && pwd` answered the caller's directory instead of the
# script's — silently, with status 0, in every script that loads a file beside itself.
mkdir -p sub
printf 'echo "lib [${BASH_SOURCE[0]}]"\n' > sub/lib.sh
printf 'echo "inner [${BASH_SOURCE[0]}]"\n. ./sub/lib.sh\n' > outer.sh
echo "top [${BASH_SOURCE[0]-unset}] ${BASH_SOURCE+set}"
. ./outer.sh
echo "back [${BASH_SOURCE[0]-unset}]"
