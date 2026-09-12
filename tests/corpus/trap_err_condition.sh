# mode: bash
# The ERR trap fires for a command that failed, under exactly the exemptions `set -e` uses, and
# once per failure however many constructs carry its status out.
trap 'echo "ERR s=$?"' ERR
false
if false; then :; fi
false || true
! true
while false; do :; done
f() { false; }
f
case x in x) false;; esac
for i in 1 2; do true; done
true | false
echo done
