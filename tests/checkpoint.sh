#!/bin/sh
# What a kernel keeps, and what it starts from. No agent is run.
#
# The database of a kernel in the store is the only one there is. With
# $JARVIS_SCRATCH set, the kernel works on a copy in scratch and saves it to
# the store as it goes. What must hold:
#
#   - a kernel that is killed, and whose scratch is gone, starts again from
#     the store with its processes and what was said, and never gives out a
#     number it may have given out before;
#   - a kernel that is stopped has saved everything;
#   - whatever is left in scratch is not looked at;
#   - what an agent keeps between turns is in the store, found again after
#     the kernel was lost, and gone when its process is removed;
#   - without scratch there is one database, in the store, and nothing is
#     copied.
#
# sh tests/checkpoint.sh. It uses its own directories under /tmp.
set -eu

here=$(cd "$(dirname "$0")/.." && pwd)
cargo build --quiet --manifest-path "$here/Cargo.toml"

work=$(mktemp -d /tmp/jv-XXXXXX)
export JARVIS_RUN="$work/run" PATH="$here/target/debug:$PATH"
export JARVIS_STORE="$work/store" JARVIS_CHECKPOINT_SECONDS=0.5
unset JARVIS_KERNEL JARVIS_ID JARVIS_SCRATCH || true
mkdir -p "$work/run" "$work/repo/harness"
cp "$here/tests/harness/echo" "$work/repo/harness/echo"
cd "$work/repo"
git init -q -b main .
mkdir .jarvis
cp "$here/tests/harness/probe-handle" .jarvis/handle
chmod +x .jarvis/handle
git add -A
git -c user.name=test -c user.email=test@example.com commit -q -m "probe: answers with code"

on() { k=$1; shift; JARVIS_KERNEL=$k jarvis "$@"; }
up() { jarvis up "$1" >> "$work/$1.log" 2>&1 & echo $! > "$work/$1.pid"; }
down() { kill "-$2" "$(cat "$work/$1.pid")" 2>/dev/null || true; }
gone() { ! kill -0 "$(cat "$work/$1.pid")" 2>/dev/null; }
finish() {
    for k in c m; do [ -f "$work/$k.pid" ] && down $k TERM; done
    sleep 1
    [ "${passed:-no}" = yes ] && rm -rf "$work" || echo "FAILED at: ${step:-the start}. What was left is in $work" >&2
}
trap finish EXIT
until_ok() {
    limit=$1; shift
    i=0
    while ! "$@" > /dev/null 2>&1; do
        i=$((i + 1))
        [ $i -gt $((limit * 5)) ] && { echo "gave up waiting for: $*" >&2; exit 1; }
        sleep 0.2
    done
}
has() { python3 -c "import json,sys; x=json.load(sys.stdin); sys.exit(0 if ($1) else 1)"; }
answered() { on "$1" messages "human@$1" --json | has "any(m['body']=='probe got: $2' for m in x)"; }
# in_store KERNEL 'text': the database in the store holds a message with that text
in_store() { python3 -c "
import sqlite3, sys
db = sqlite3.connect('file:$JARVIS_STORE/kernel/$1/kernel.db?mode=ro', uri=True)
sys.exit(0 if db.execute('select count(*) from messages where body = ?', (sys.argv[1],)).fetchone()[0] else 1)" "$2"; }
restored() { on "$1" log --json -n 100000 | python3 -c "import json,sys; print(len([f for f in json.load(sys.stdin) if f['kind']=='restored']))"; }
number_of() { python3 -c "import json,sys; print(json.load(sys.stdin)['id'].split(':')[1])"; }
# What the test harness keeps for its agent: how many turns it has had.
kept_turns() { cat "$JARVIS_STORE/echo/$1/turns"; }
more_turns_than() { [ "$(kept_turns "$1")" -gt "$2" ]; }

step="a kernel with scratch is in the store from its first moment"
export JARVIS_SCRATCH="$work/scratch"
up c
until_ok 20 on c kernels
[ -f "$JARVIS_STORE/kernel/c/kernel.db" ]
[ -f "$JARVIS_SCRATCH/kernel/c/kernel.db" ]
[ "$(on c spawn probe@c --from main --harness none)" = "probe@c" ]
[ -d "$JARVIS_STORE/kernel/c/proc/probe" ]
[ "$(on c spawn agent@c --harness echo --prompt 'A test process.')" = "agent@c" ]
until_ok 20 more_turns_than agent@c 0
[ ! -e "$JARVIS_SCRATCH/echo" ]
before=$(on c send --json probe@c "ping one" | number_of)
until_ok 20 answered c "ping one"
until_ok 20 in_store c "probe got: ping one"
echo "the kernel works in scratch; its database, its processes' code and what an agent keeps are in the store"

step="killed, scratch gone: it starts from the store"
down c KILL
until_ok 10 gone c
rm -rf "$JARVIS_SCRATCH"
up c
until_ok 20 on c kernels
on c ps --json | has "{p['id'] for p in x} == {'probe@c', 'agent@c'}"
had=$(kept_turns agent@c)
on c send agent@c "hello again" > /dev/null
until_ok 20 more_turns_than agent@c "$had"
answered c "ping one"
[ "$(restored c)" = 1 ]
after=$(on c send --json probe@c "ping two" | number_of)
[ "$after" -gt $((before + 1000000)) ]
until_ok 20 answered c "ping two"
echo "it came back with its processes and what was said, and numbers on from $after (it was at $before)"
echo "its agent found what it had kept, and carried on from turn $had"

step="a removed process: what its agent kept goes with it"
on c remove agent@c > /dev/null
[ ! -e "$JARVIS_STORE/echo/agent@c" ]
echo "removed, and nothing of its agent is left in the store"

step="stopped: everything is saved, and what is left in scratch is not looked at"
down c TERM
until_ok 20 gone c
in_store c "probe got: ping two"
# Something else in the place of the working database: it must not be used.
: > "$JARVIS_SCRATCH/kernel/c/kernel.db"
up c
until_ok 20 on c kernels
answered c "ping two"
[ "$(restored c)" = 1 ]
down c TERM
until_ok 20 gone c
echo "after a stop nothing was lost, and it did not skip its numbers again"

step="without scratch there is one database, in the store"
unset JARVIS_SCRATCH
up m
until_ok 20 on m kernels
[ "$(on m spawn probe@m --from main --harness none)" = "probe@m" ]
on m send probe@m "ping three" > /dev/null
until_ok 20 answered m "ping three"
[ "$(find "$work" -name kernel.db -path '*kernel/m/*' | wc -l | tr -d ' ')" = 1 ]
[ -f "$JARVIS_STORE/kernel/m/kernel.db" ]
down m KILL
until_ok 10 gone m
up m
until_ok 20 on m kernels
answered m "ping three"
[ "$(restored m)" = 0 ]
echo "a kernel without scratch works in the store, and a kill loses nothing"

passed=yes
echo "PASSED"
