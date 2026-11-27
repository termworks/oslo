# mode: bash
# `cd //` keeps exactly two leading slashes, an empty OLDPWD is a directory `cd -` can go to,
# `cd -e` is accepted, and `pushd -n` stores its operand as written.
start=$PWD
cd //
pwd
cd ///
pwd
cd //tmp
pwd
cd "$start"
OLDPWD=
cd -
echo "empty oldpwd rc=$?"
cd -e /
echo "cd -e rc=$?"
cd "$start"
mkdir -p a b
pushd -n a | sed "s|$HOME|~|g; s|$start|.|g"
dirs -l | sed "s|$start|.|g"
pushd a >/dev/null
pushd ../b >/dev/null
dirs -l | sed "s|$start|.|g"
