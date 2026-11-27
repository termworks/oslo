# mode: bash
# The ERR trap sees the failing command's line in `$LINENO` and its text in `$BASH_COMMAND`.
trap 'echo "ERR line $LINENO: $BASH_COMMAND"' ERR
x=1
false 1 2
y=2
f() {
  false
}
f
echo done
