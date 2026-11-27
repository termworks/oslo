# mode: bash
# The case attributes convert every assignment, and the last one given wins.
declare -l lo=ABC
lo+=DeF
echo "$lo"
declare -u up=abc
read -r up <<< "hello"
echo "$up"
declare -c cap="hello wORLD"
echo "$cap"
declare -l both
declare -u both
both=MiXeD
declare -p both
declare -u ss="straße"
echo "$ss"
declare -t tr=1
declare -p tr
declare +t tr
declare -p tr
# `-i` on an array evaluates every element, and `+=` on one adds.
declare -ai a=(1+1 2*3)
a+=(4+4)
a[5]=2**3
a[0]+=10
echo "${a[@]}"
declare -p a
declare -il il=AB
echo "$il"
# The integer attribute belongs to the binding, so `unset` takes it away.
declare -i n=5
unset n
n=2+3
declare -p n
declare -i k=3
declare +i k
k=1+1
echo "$k"
# `-a` with a scalar value makes an array; so does `export` of a literal.
declare -a one=1
declare -p one
export arr=(1 2)
declare -p arr
env | grep -c '^arr=' || true
# `+x` takes the export away again.
ex=1
export ex
declare +x ex
declare -p ex
env | grep -c '^ex=' || true
# The refusals: `+r` on a read-only name, `+a` on an array.
readonly ro=1
declare +r ro 2>/dev/null || echo "+r: $?"
b=(1 2)
declare +a b 2>/dev/null || echo "+a: $?"
declare -p ro b
# `readonly` takes array and function flags too.
readonly -a ra=(1 2)
readonly -A rA=([k]=v)
declare -p ra rA
f() { :; }
readonly -f f
f() { echo redefined; } 2>/dev/null
echo "redefine: $?"
unset -f f 2>/dev/null || echo "unset -f: $?"
declare -F f
declare -F | grep ' f$'
# `declare -g` inside a function writes the global even past a local of the same name.
x=1
g() { local x=2; declare -g x=3; echo "in g: $x"; declare -gi gi; gi=4+4; }
g
echo "after g: $x $gi"
