# mode: bash
# `compgen -G`, `-W`, `-f` and `-d`: what scripts and completion functions call. oslo had no
# `compgen` at all, so every one of these was `command not found`.
touch a1 b1
mkdir sub
compgen -G 'a*'
compgen -W 'alpha beta apple' a
compgen -d
compgen -d su
compgen -f a
compgen -G 'zz*'
echo "rc=$?"
