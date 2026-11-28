# mode: bash
# A process substitution in an assignment value or an array element is expanded like any other.
x=<(true); echo "$x"
y=a<(true)b; echo "$y"
declare z=<(true); echo "$z"
w=(<(true) x <(true)); echo "${w[@]}"
v=$(cat <(echo inner)); echo "$v"
