# mode: bash
# `${x@op}`: the parameter transformations, on scalars, array elements, arrays and `$@`.

n() { printf '%s:' "$#"; printf '[%s]' "$@"; echo; }

x=abc; y="it's"; z=$'a\nb'; e=
echo ${x@Q} ${y@Q} ${z@Q} "[${e@Q}]" "[${unset@Q}]"
w=$'tab\there'; q=$'\x01'; m=$'it\x27s\n'; esc=$'\e[0m'; dq='a"b'; t='$x`'
echo ${w@Q} ${q@Q} ${m@Q} ${esc@Q} ${dq@Q} ${t@Q} 'a*?[]~#'
s='a\tb\x41\101\cA'
printf '%s' "${s@E}" | od -An -c
v=hello; V=HELLO
echo ${v@U} ${v@u} ${V@L} ${v@K} ${v@k}

declare -i i=3
declare -r r=2
export ex=1
declare -rix rix=1
a=(1 'x y')
echo "[${x@a}] [${i@a}] [${r@a}] [${ex@a}] [${rix@a}] [${a@a}] [${a[1]@a}]"
echo "${x@A}|${i@A}|${ex@A}|${rix@A}|${y@A}|${z@A}"
echo "${a[1]@A}|${a@A}|[${unset@A}]"

n "${a[@]@Q}"
n "${a[*]@Q}"
n ${a[@]@Q}
n "${a[@]@K}"
n "${a[@]@k}"
n "${a[@]@A}"
n "${a[@]@a}"
n "${a[@]@U}"
b=($'a\nb' 'q"r' 'x$y')
echo "${b[@]@A}"
echo "${b[@]@K}"

set -- p 'q r'
n "${@@Q}" "${*@Q}"
n "${@@A}"
n "${@@a}"
n "${@@K}"
n ${@@U}
n "${1@A}" "${1@Q}"

ref=x
echo ${!ref@Q} ${!ref^^}
echo "[${unset@Z}]"
( : "${x@Z}"; echo unreachable ) 2>/dev/null; echo "unknown letter: $?"

HOME=$PWD
p='\w|\$|\\|\a\e\[x\]|${HOME:+home}|$(echo hi)|$((1+2))'
printf '%s' "${p@P}" | od -An -c
cd /
p='\w \W'
echo "${p@P}"

echo $[1+2] "$[x=4, x*2]" $x
