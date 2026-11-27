# mode: bash
cat > lib.sh <<'EOF'
libf() {
  echo "libf: $LINENO ${BASH_SOURCE[*]} ${BASH_LINENO[*]} ${FUNCNAME[*]}"
  caller; echo "st=$?"
  caller 0; echo "st=$?"
  caller 1; echo "st=$?"
  caller 2; echo "st=$?"
  caller 3; echo "st=$?"
}
echo "lib: $LINENO ${BASH_SOURCE[*]} ${BASH_LINENO[*]} [${FUNCNAME[*]}]"
caller; echo "st=$?"
caller 0; echo "st=$?"
EOF
cat > main.sh <<'EOF'
echo "top: $LINENO ${BASH_SOURCE[*]} ${BASH_LINENO[*]} [${FUNCNAME[*]}] ${#BASH_SOURCE[@]}"
caller; echo "st=$?"
g() {
  libf
  source ./lib.sh
}
source ./lib.sh
h() { g; }
h
eval 'echo "eval: $LINENO"
echo "eval: $LINENO"'
x=$(echo "sub: $LINENO ${FUNCNAME[*]-none}"); echo "$x"
k() { echo "k: ${FUNCNAME[*]} ${BASH_LINENO[*]} ${BASH_SOURCE[*]}"; }
k | cat
(k)
m() {
  if true; then
    echo "m: $LINENO"
  fi
  eval 'echo "m eval: $LINENO"'
}
m
EOF
"$BASH" ./main.sh
