# mode: bash
# Assigning to a read-only name abandons the rest of the line it is on — the whole list, the
# compound around it, the function it was called from — and the next line still runs.
readonly x=1
x=2; echo "same line"
echo "next line: $?"
{ x=2; echo "inside a group"; }; echo "after the group"
echo "after group line"
f() { x=2; echo "inside f"; }
f; echo "after f"
echo "after f line"
x=2 && echo "and"
x=2 || echo "or"
for i in 1; do x=2; echo "loop"; done; echo "after loop"
if x=2; then echo "then"; fi
x+=3; echo "append"
readonly -a ra=(1 2)
ra[0]=x; echo "element"
ra+=(3); echo "array append"
declare -p ra
# A subshell, `eval`, `source` and a command substitution each end with status 1 instead.
(x=2; echo "subshell"); echo "after subshell $?"
eval 'x=2; echo "in eval"'; echo "after eval $?"
printf 'x=2\necho "in source"\n' > script.sh
. ./script.sh; echo "after source $?"
v=$(x=2; echo "in substitution"); echo "after substitution [$v]"
x=2 | cat; echo "after pipe"
# A prefix assignment to a read-only name is reported, and the command still runs.
x=2 echo "prefixed"
# Builtins that assign report a failure and carry on.
declare x=2; echo "declare $?"
read x <<< 5; echo "read $?"
export x=2; echo "export $?"
for x in a; do echo "never"; done; echo "for $?"
: "${x:=2}"; echo "default $x"
# A subscript before the start of an array abandons the line too.
a=(1 2 3)
a[-5]=q; echo "bad subscript"
echo "status $?"
b=([-1]=x); echo "bad literal"
declare -p a
