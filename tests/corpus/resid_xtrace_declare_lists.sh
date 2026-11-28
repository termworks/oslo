# mode: bash
exec 2>&1
set -x
declare -a a=(1 2)
declare -A m=([k]=v)
f() { local -a b=(x "y z"); local c=3; }
f
export e=(1)
readonly -a r=(q)
typeset -a t=()
declare -a s="(7 8)"
declare -a u=(1) v=2 w=(3)
declare -a o+=(9)
declare -a a2=([3]=x [5]="y z" w)
declare -A m2=([k k]=v [j]="it's")
export z=1 e2=(1)
