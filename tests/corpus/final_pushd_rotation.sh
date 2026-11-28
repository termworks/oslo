# mode: bash
# `pushd +N` and `-N` turn the stack as bash does: the entry that comes to the top is the one
# `cd` goes to, and it is dropped when `-n` stays put or the move fails. Only a move prints.
HOME=$PWD
mkdir a b c
pushd -n a >/dev/null
pushd -n b >/dev/null
pushd -n c >/dev/null
dirs
pushd -n +1
dirs
pushd -n -0
pushd -n +2
dirs
pushd +0
pushd a
pushd ../b
pushd -0 2>/dev/null
echo "rc=$?"
dirs
pushd +1
echo "rc=$?"
