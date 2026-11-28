# mode: bash
# ${r:=v} on a read-only name abandons the command with 2 where bash keeps the status, 1 elsewhere
readonly r
(echo ${r:=2}; echo same) 2>/dev/null; echo "subshell: $?"
(: "${r=2}") 2>/dev/null; echo "unset form: $?"
x=$(echo ${r:=2}; echo same) 2>/dev/null; echo "substitution: $?"
{ echo ${r:=2}; echo same; } 2>/dev/null | cat; echo "group stage: ${PIPESTATUS[0]}"
echo ${r:=2} 2>/dev/null | cat; echo "simple stage: ${PIPESTATUS[0]}"
{ echo ${r:=2}; } 2>/dev/null & wait $!; echo "job: $?"
f() { echo ${r:=2}; echo in; }
(f) 2>/dev/null; echo "function: $?"
(eval 'echo ${r:=2}; echo same'; echo after) 2>/dev/null; echo "eval in subshell: $?"
(eval 'echo ${x!!}; echo same'; echo after) 2>/dev/null; echo "bad substitution in eval: $?"
(eval 'readonly q=1; q=2; echo same'; echo after) 2>/dev/null; echo "refused assignment in eval: $?"
y=$(eval 'echo ${x!!}'; echo after) 2>/dev/null; echo "eval in substitution: [$y] $?"
eval 'echo ${r:=2}; echo same' 2>/dev/null; echo "eval: $?"
