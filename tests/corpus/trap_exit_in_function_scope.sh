# mode: bash
# `exit` inside a function runs the EXIT trap before unwinding, so the trap sees that function's
# locals, `FUNCNAME` and positional parameters.
set -- a b
main() {
  local tmp=scratch
  set -- z
  trap 'echo "tmp=$tmp args=$# $1 frames=${FUNCNAME[*]}"' EXIT
  exit 3
}
main
echo not reached
