# mode: bash
for b in cd printf alias declare read set trap type ulimit wait let local source . kill caller exit return shift help mapfile; do
  "$b" --help >/dev/null
  echo "$b $?"
done
builtin cd --help | head -1
command printf --help | head -1
echo --help
test --help; echo "test $?"
true --help; echo "true $?"
cd -z; echo "cd $?"
