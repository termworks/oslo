# mode: bash
f() {
  local x='a b' y
  cat <<EOF
body $x
EOF
  [[ $x == a* ]] && echo glob
  [[ $x != "a*" ]] && echo literal
  y=$(echo "\$x" "q\"q")
  echo "$y" {1,2} ${!z-unset}
  echo out |& cat
}
z=x
f
eval "$(declare -f f | sed 's/^f ()/g ()/')"
g
declare -f f | tr -d -c '\001\002' | wc -c
