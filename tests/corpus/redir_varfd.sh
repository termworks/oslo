# mode: bash
# `{name}>file` and friends open a descriptor from 10 up, store its number in `name`, and leave it
# open after the command; `{name}>&-` closes the descriptor `name` holds.
exec {fd}>out.txt
echo "high=$(( fd >= 10 ))"
echo written >&"$fd"
exec {fd}>&-
cat out.txt
echo again >&"$fd" 2>/dev/null
echo "closed st=$?"

echo data >in.txt
exec {r}<in.txt
read -r -u "$r" line
echo "read=$line"
exec {r}<&-

echo hi {w}>cmd.txt
echo "kept=$(( w >= 10 ))"
echo more >&"$w"
exec {w}>&-
cat cmd.txt

exec {rw}<>rw.txt
echo both >&"$rw"
exec {rw}>&-
cat rw.txt

exec {d}>&1
echo "via dup" >&"$d"
exec {d}>&-

f() { local fd; exec {fd}>f.txt; echo "in function" >&"$fd"; exec {fd}>&-; }
f
cat f.txt

pair=()
exec {pair[1]}>arr.txt
echo element >&"${pair[1]}"
exec {pair[1]}>&-
cat arr.txt

unset nothing
exec {nothing}>&- 2>/dev/null
echo "unset st=$?"
