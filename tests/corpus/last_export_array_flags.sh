# mode: bash
# `export -a`/`-A` make an assigned value an array and narrow a listing to arrays.
export -A m=([a]=1)
declare -p m
export -a z=(1 2)
declare -p z
export -a x=1
declare -p x
y=5
export -a y
declare -p y
export -A u
declare -p u
declare -A q=([k]=v)
w=(1)
s=1
export q w s
export -a
export -A
v=7
readonly -a v
declare -p v
readonly -A r
declare -p r
readonly -A rr=([a]=1)
declare -p rr
