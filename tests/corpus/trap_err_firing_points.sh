# mode: bash
# Which failures run the ERR trap: simple commands, calls, subshells, `(( ))` and pipelines —
# not the compounds that carry an inner failure out.
trap 'echo "ERR [$BASH_COMMAND]"' ERR
eval false
eval 'false; true'
{ false; }
false | false
((0))
[[ a = b ]] && echo no
x=$(false)
g() { false; return 3; }
g
for i in 1; do false; done
if true; then false; fi
! false
false && true
false || true
while false; do :; done
echo end
