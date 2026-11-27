# mode: bash
# The menu is laid out in columns that fit $COLUMNS, numbered down each column, padded with tabs.
COLUMNS=80
select x in {1..25}; do :; done 2>wide.txt </dev/null
cat wide.txt
COLUMNS=40
select x in aaaa bbbbbbbbbbbbb c d e f g h i j k l; do :; done 2>narrow.txt </dev/null
cat narrow.txt
COLUMNS=500
select x in {1..12}; do :; done 2>one.txt </dev/null
cat one.txt
