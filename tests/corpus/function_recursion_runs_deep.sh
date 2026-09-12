# mode: bash
# A shell function may recurse further than a hundred levels, which is what a tree walk or a
# recursive-descent parser written in shell actually needs. bash and dash both run this; oslo used
# to answer `maximum nesting level exceeded` at 100 and stop, alone among the three.
down() {
    if [ "$1" -eq 0 ]; then
        echo "bottom"
        return 0
    fi
    down $(( $1 - 1 ))
}
down 500
echo "rc=$?"

# The counter comes back down with the recursion: a second run has the whole depth again, rather
# than starting from wherever the first one left the count.
down 500
echo "rc=$?"

# It unwinds through an early return too, which is where a paired counter goes wrong first.
early() {
    if [ "$1" -eq 0 ]; then
        return 7
    fi
    early $(( $1 - 1 ))
}
early 400
echo "early=$?"
down 500
echo "rc=$?"
