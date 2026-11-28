# mode: bash
# `n>&m-` and `n<&m-` move a descriptor: n becomes a copy of m, and m is closed.
exec 3>out.txt
echo via3 >&3
exec 4>&3-
echo via4 >&4
echo lost >&3 2>/dev/null
echo "closed st=$?"
exec 4>&-
cat out.txt

exec 3>moved.txt
echo moved 1>&3-
echo "cmd st=$?"
echo after >&3
exec 3>&-
cat moved.txt

echo data >in.txt
exec 5<in.txt
cat 0<&5-
echo "in st=$?"

exec 7>&9- 2>/dev/null
echo "bad st=$?"
