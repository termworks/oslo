# mode: bash
RANDOM=42; echo $RANDOM $RANDOM $RANDOM
RANDOM=0; echo $RANDOM $RANDOM
RANDOM=-1; echo $RANDOM
RANDOM=4294967296; echo $RANDOM
RANDOM=3*2; a=$RANDOM; RANDOM=6; b=$RANDOM; [[ $a == "$b" ]] && echo arithmetic-seed
RANDOM=7; a=$(echo $RANDOM $RANDOM $RANDOM); b=$(echo $RANDOM $RANDOM $RANDOM)
[[ $a != "$b" ]] && echo reseeded-per-substitution
RANDOM=7; (RANDOM=9; echo $RANDOM); echo $RANDOM
unset RANDOM; RANDOM=2+3; echo $RANDOM
