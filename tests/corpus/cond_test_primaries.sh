# mode: bash
# `test` with three words joins two strings with a middle `-a` or `-o` whatever they spell, `-t`
# wants an integer, `-N` asks for a modification later than the last access, and `[[ ]]` leaves
# `$_` alone.

[ ! -a / ]; echo "! -a /: $?"
[ -z -a -z ]; echo "-z -a -z: $?"
[ -n -a -n ]; echo "-n -a -n: $?"
[ "" -o -n ]; echo "'' -o -n: $?"
[ "" -a x ]; echo "'' -a x: $?"
[ ! -z / ]; echo "! -z /: $?"
[ ! = ! ]; echo "! = !: $?"

[ -t abc ] 2>/dev/null; echo "-t abc: $?"
[ -t ]; echo "-t alone: $?"
[ -t 99 ]; echo "-t 99: $?"
test -t 1x 2>/dev/null; echo "-t 1x: $?"
[[ -t abc ]] 2>/dev/null; echo "[[ -t abc ]]: $?"

touch f
[ -N f ]; echo "-N after touch: $?"
[[ -N f ]]; echo "[[ -N ]] after touch: $?"
touch -a -d '2000-01-01' f
[ -N f ]; echo "-N after an old read: $?"

true
[[ x == x ]]; echo "after [[: $_"
echo a b
[[ -n foo ]]; echo "still: $_"
[ x ]; echo "after [: $_"
