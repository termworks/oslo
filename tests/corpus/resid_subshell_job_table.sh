# mode: bash
sleep 1 &
echo "[$(jobs)]"
( jobs ); echo "sub=$?"
{ jobs; } | cat
jobs | cat
echo "[$( (jobs) )]"
echo "[$(jobs -p | wc -l)]"
kill %1
wait
