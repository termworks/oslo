# mode: bash
f1() { echo hi; }
f2() { echo a; echo b; echo c; }
f3() {
  for i in 1 2 $(seq 3); do echo $i; done
  for ((i=0;i<2;i++)); do :; done
  for x; do echo "$x"; done
  while read -r l; do echo "$l"; done < file
  until false; do break; done
}
f4() {
  if [[ $1 == a* ]]; then echo a; elif [ "$1" = b ]; then echo b; else echo c; fi
  case $1 in a|b) echo ab ;; c) ;& d) echo cd ;;& *) echo star; esac
}
f5() { ( cd /tmp; ls ) | grep x && echo y || echo z; { echo g1; echo g2; } > /dev/null 2>&1; }
f6() { local x=1 y; x+=2; a=(1 "2 3" $x); declare -a b=(q r); echo ${#a[@]} ${a[1]} ${!x} ${x:-d} "${x//a/b}"; }
f12() ( echo sub; echo shell )
f13() { g() { echo inner; }; g; function h { :; }; }
f14() { echo hi; } > out 2>&1
function f15 { echo kw; }
f16() { if true; then :; fi; while :; do break; done; for i in a; do :; done; }
f18() { a && { b; c; }; d || ( e ); }
f19() { echo 1;
# comment
echo 2   # trailing
}
f20() { if a; b; then c; fi; while a; b; do c; done; }
f21() { a &
b
c & d; }
declare -f f1 f2 f3 f4 f5 f6 f12 f13 f14 f15 f16 f18 f19 f20 f21
type f1
command -V f2
