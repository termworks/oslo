# mode: bash
# A lone condition resets its trap, a lone action is a usage error, the four conditions that are
# not signals take no `SIG` prefix, and `trap -p NAME…` answers in the order asked.
trap 'echo x' USR1
trap USR1
trap -p USR1
trap 'echo x' 2>/dev/null
echo "lone action: $?"
trap 'echo x' SIGEXIT 2>/dev/null
echo "SIGEXIT: $?"
trap 'echo x' SIGERR 2>/dev/null
echo "SIGERR: $?"
trap 'echo e' ERR
trap 'echo r' RETURN
trap 'echo i' INT
trap 'echo z' EXIT
trap -p RETURN INT ERR EXIT
trap
trap - EXIT
