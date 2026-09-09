# mode: bash
# `. file a b` gives the sourced file its own positional parameters, and the caller's come back
# afterwards. Bare `. file` leaves the caller's in place instead.
set -- outer1 outer2
printf 'echo "in=[$*] one=[$1] n=$#"\n' > lib.sh
. ./lib.sh a b c
echo "after=[$*] n=$#"
. ./lib.sh
echo "again=[$*] n=$#"
