# mode: bash
eval 'echo A
fi
echo B'
echo "ev=$?"
printf 'echo S1\nfi\necho S2\n' > s.sh
. ./s.sh
echo "src=$?"
printf 'echo T1\nif true; then\n' > t.sh
source ./t.sh
echo "src=$?"
