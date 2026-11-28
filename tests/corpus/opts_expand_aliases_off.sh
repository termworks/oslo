# mode: bash
# A shell that is not interactive does not expand aliases until `expand_aliases` is set.
alias e='echo expanded'
e 2>/dev/null || echo "not expanded"
shopt -s expand_aliases
alias two='echo one
echo two'
e
two
shopt -u expand_aliases
e 2>/dev/null || echo "off again"
