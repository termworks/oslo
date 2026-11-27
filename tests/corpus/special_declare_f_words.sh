# mode: bash
f7() { cat <<EOF
hello $x
EOF
cat <<'E2' | wc -l
raw $y
E2
echo after
}
f8() { echo "\$y" "a\"b" 'q' \* $'a\tb' {a,b} ~/x; ls |& cat; time sleep 0; ! true; sleep 1 & wait; }
f9() { (( x = 1 + 2 )); let y=3; [[ -n $x && ( a < b || ! -f c ) ]]; [[ x =~ ^"$R"$ ]]; [[ abc == a* ]]; }
f10() { echo $(ls   -l;  echo  x) `echo bt` <(echo p) "$(echo "in")"; x=$(if true; then echo y; fi); }
f11() { echo a >out; echo b >>out; cat <out; echo c 2>/dev/null; echo d >&2; exec 3<&-; echo e 1>&2 2>&3; echo f &>/dev/null; cat <<<"$x"; }
f17() { x=1 y=2 cmd arg; echo "a
b"; echo $'x\ny'; }
g1() { cat <(ls   -l) >(wc  -c); echo ${x:-$(echo   d)} "${y:-$(ls  ;  pwd)}" $((1+$(echo  2))); }
g2() { a=(  1   2   3 ) ; b=( [0]=x [5]="y z" ); c+=( q ); echo $"loc" $'it\'s' "$(echo a
echo b)"; }
g3() { cat <>f; cat 3<>f; echo x >&$fd; echo y 2>&$fd; cat <&3; cat <&-; exec 4>&3-; echo z >|f; cat 0<f; echo w 1>f; }
g4() { for (( i = 0 ; i < 2 ; i++ )); do :; done; for ((;;)); do break; done; for x in ; do :; done; }
g5() { cat <<-EOF
	tabbed
	EOF
cat <<"Q" <<R
q
Q
r
R
}
g6() { echo $( (echo a) ) $(echo a & ) $(case x in x) echo y;; esac); x=`echo a; echo b`; }
g8() { echo a \
  b; [[ a == b ]] || [[ $(echo x) ]]; [[ ! $y == z ]]; }
g9() { ! a | b; time ! c; time -p d; a | b | c; a |& b |& c; }
h1() { x=$(case $y in a) echo 1;; b|c) echo 2;; esac; echo z); }
h3() { x=$(for i in 1 2; do echo $i; done); y=$(while :; do break; done); z=$({ a; b; }); }
declare -f f7 f8 f9 f10 f11 f17 g1 g2 g3 g4 g5 g6 g8 g9 h1 h3
