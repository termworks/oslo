# mode: posix
x=; echo ${!x}; echo same
echo "next $?"
echo $((1+)); echo same
echo never
