# mode: bash
# A `select` header gets the DEBUG trap and the trace line once, as `for` and `case` do.
trap 'echo "D[$BASH_COMMAND]"' DEBUG
select x in a b; do break; done 2>/dev/null
trap - DEBUG
