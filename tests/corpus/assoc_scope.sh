# mode: bash
# `local -A` in a function, a function writing into a caller's map, and copying one map.
declare -A conf=([host]=localhost [port]=80)
f() {
  local -A conf=([inner]=1)
  conf[extra]=2
  echo "in f: ${#conf[@]} ${conf[inner]} [${conf[host]}]"
  declare -p conf
}
f
echo "after f: ${conf[host]} ${conf[port]} ${#conf[@]}"
g() { conf[port]=8080; }
g
echo "after g: ${conf[port]}"
# A declaration inside a function is local whether or not -A comes with a value.
h() { declare -A conf; conf[x]=1; echo "h: ${!conf[@]}"; }
h
echo "${!conf[@]}" | tr ' ' '\n' | sort | tr '\n' ' '
echo
# Copying key by key, and the copy is independent of the original.
declare -A copy
for k in "${!conf[@]}"; do declare "copy[$k]=${conf[$k]}"; done
declare -p copy
declare 'copy[port]=1'
echo "${conf[port]} ${copy[port]}"
# A map is unset as a whole, and gone from a function's view afterwards.
unset conf
declare -p conf 2>/dev/null || echo "no conf"
# An indexed array cannot become associative, nor the other way round.
declare -a ix=(1 2)
declare -A ix 2>/dev/null || echo "refused: $?"
declare -A as=([k]=v)
declare -a as 2>/dev/null || echo "refused: $?"
declare +A as 2>/dev/null || echo "refused: $?"
declare -p ix as
