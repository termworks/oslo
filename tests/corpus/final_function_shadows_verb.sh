# mode: bash
# A shell function outranks every name oslo gives a structured verb, alone and in a pipeline.
for v in parse lines from detect-columns where to first final length cols get sort-by \
    each map reverse count distinct group-by stats reject rename skip every lookup explore; do
    eval "$v() { echo \"$v:\$#:\$*\"; }"
done
parse -a -b rest1 rest2
lines x y
from json
detect-columns --skip 1
echo in | parse '{a}' b
printf 'a\nb\n' | lines | cat
printf '[1]' | from json | length
where 'size > 1' x
first 2 | to json
final 1 2 3 | count | distinct
get name | sort-by name | reverse
each 'print(1)' | map x | cols a b
group-by x | stats y | reject z | rename a b | skip 1 | every 2 | lookup k | explore

ls() { echo "ls:$#"; }
df() { echo "df:$#"; }
ps() { echo "ps:$#"; }
ls | where name
df | first 1
ps a b | length
unset -f ls df ps

shopt -s expand_aliases
unset -f to first length lines
alias to='echo to-alias' first='echo first-alias' length='echo length-alias' lines='echo lines-alias'
printf 'x\n' | lines | length
echo q | to json
echo q | first 2 | cat
