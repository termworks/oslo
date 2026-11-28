# mode: bash
# bash expands an alias before it looks for reserved words, so an alias may be named `if`.
shopt -s expand_aliases
alias if='echo IF' while='echo WHILE' then='echo THEN' do='echo DO'
if x; echo y
while 1; then 2
do 3
unalias if while then do
if true; then echo restored; fi
alias fi='echo FI'
fi z
unalias fi
eval 'alias until="echo UNTIL"'
until q
