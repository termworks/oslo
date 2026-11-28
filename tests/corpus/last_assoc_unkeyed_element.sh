# mode: bash
# a bare element after a keyed one in an associative literal is refused before redirections
{ declare -A m=([a]=1 b) 2>&1; echo "same line"; } 2>/dev/null | cat
echo "after declare: ${PIPESTATUS[0]}"
( declare -A m=([a]=1 b) 2>&1; echo "same line" ) 2>/dev/null; echo "subshell: $?"
f() { local -A l=([a]=1 "b c") 2>&1; echo "in function"; }
( f ) 2>/dev/null; echo "local: $?"
declare -A n
( n=([a]=1 b); echo "same line" ) 2>/dev/null; echo "assignment: $?"
n=(b [a]=1 c)
declare -p n
declare -A p=(x [y]+=2 z)
declare -p p
declare -A q="([a]=1 b)" 2>/dev/null; echo "string form: $?"
declare -p q
( declare -A r=([a]=1 [b]) ) 2>/dev/null; echo "bracket without value: $?"
