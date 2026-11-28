# mode: bash
# A compound command leaves the `PIPESTATUS` its last command published; a simple command, a
# subshell and `(( ))` publish their own.
true; if false; then :; fi; echo "if ${PIPESTATUS[*]}"
while false; do :; done; echo "while ${PIPESTATUS[*]}"
{ false; }; echo "group ${PIPESTATUS[*]}"
case x in x) false;; esac; echo "case ${PIPESTATUS[*]}"
false; (exit 3); echo "subshell ${PIPESTATUS[*]}"
true; ((0)); echo "arith ${PIPESTATUS[*]}"
h() { false | true; }; h; echo "call ${PIPESTATUS[*]}"
false; f() { :; }; echo "definition ${PIPESTATUS[*]}"
false; ! true; echo "negated ${PIPESTATUS[*]}"
