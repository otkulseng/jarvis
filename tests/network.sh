#!/bin/sh
# A network of three kernels on this machine, a - b - c, and what must hold
# in it. No agent is run: tests/harness/echo answers every message.
#
#   deliver  a message crosses kernels that are not linked directly; while
#            the kernel in the middle is killed again and again, every
#            message still arrives, arrives once, and its sender's kernel
#            learns that it was dealt with;
#   request  a process is created, stopped and started on another kernel,
#            also when that kernel cannot be reached at the time;
#   ask      one kernel shows the processes, messages and records of the
#            others, and names a kernel that does not answer;
#   follow   one kernel receives the others' changes as they happen, and
#            carries on where it stopped after an outage;
#   also     a process without an agent, a second way round, names that are
#            refused, a link that fails, a kernel taken to be gone.
#
# Run it from anywhere: sh tests/network.sh. It uses its own repository and
# sockets under /tmp and takes about a minute.
set -eu

here=$(cd "$(dirname "$0")/.." && pwd)
cargo build --quiet --manifest-path "$here/Cargo.toml"

# Short, because a socket's path must be.
work=$(mktemp -d /tmp/jv-XXXXXX)
export JARVIS_RUN="$work/run" PATH="$here/target/debug:$PATH"
unset JARVIS_KERNEL JARVIS_ID || true
mkdir -p "$work/run" "$work/repo/harness"
cp "$here/tests/harness/echo" "$work/repo/harness/echo"
cd "$work/repo"
git init -q -b main .
git add -A
git -c user.name=test -c user.email=test@example.com commit -q -m "a repository for the test"
# An expert whose processes need no agent: its code answers.
git checkout -q --orphan probe
git rm -rf -q .
mkdir .jarvis
cp "$here/tests/harness/probe-handle" .jarvis/handle
chmod +x .jarvis/handle
git add -A
git -c user.name=test -c user.email=test@example.com commit -q -m "probe: answers with code"
git checkout -q main

on() { k=$1; shift; JARVIS_KERNEL=$k jarvis "$@"; }      # on KERNEL COMMAND...: give a command to that kernel
as() { who=$1; shift; JARVIS_KERNEL=${who#*@} JARVIS_ID=$who jarvis "$@"; }   # as ADDRESS COMMAND...: as that process
up() { jarvis up "$1" >> "$work/$1.log" 2>&1 & echo $! > "$work/$1.pid"; }
down() { kill "-$2" "$(cat "$work/$1.pid")" 2>/dev/null || true; }
finish() {
    [ -n "${watcher:-}" ] && { kill "$watcher" 2>/dev/null; wait "$watcher" 2>/dev/null; } || true
    for k in a b c; do down $k TERM; done
    sleep 1
    [ "${passed:-no}" = yes ] && rm -rf "$work" || echo "FAILED at: ${step:-the start}. What was left is in $work" >&2
}
trap finish EXIT
# until_ok SECONDS COMMAND...: wait until the command succeeds
until_ok() {
    limit=$1; shift
    i=0
    while ! "$@" > /dev/null 2>&1; do
        i=$((i + 1))
        [ $i -gt $((limit * 5)) ] && { echo "gave up waiting for: $*" >&2; exit 1; }
        sleep 0.2
    done
}
# has 'python expression over x': read JSON from standard input, succeed if the expression is true
has() { python3 -c "import json,sys; x=json.load(sys.stdin); sys.exit(0 if ($1) else 1)"; }
count() { python3 -c "import json,sys; print(len([m for m in json.load(sys.stdin) if $1]))"; }
reaches() { on "$1" kernels --json | has "any(k['name']=='$2' and k['reachable'] for k in x)"; }
state_of() { on "$1" ps "$2" --json 2>/dev/null | has "x['state']=='$3'"; }
refused() { if "$@" > /dev/null 2>&1; then echo "was not refused: $*" >&2; exit 1; fi; }
# sees 'python expression over lines': what the watcher on a has received so far
sees() {
    python3 -c "
import json, sys
lines = [json.loads(l) for l in open(sys.argv[1]) if l.strip().startswith('{')]
sys.exit(0 if ($1) else 1)" "$work/watch.jsonl"
}

step="starting three kernels"
for k in a b c; do up $k; done
for k in a b c; do until_ok 10 on $k kernels; done
on a link b > /dev/null
on b link c > /dev/null
until_ok 20 reaches a c
until_ok 20 reaches c a
echo "three kernels, linked a - b - c"

step="creating processes, one of them on another kernel"
on a spawn asker@a --harness echo --prompt "A test process." > /dev/null
[ "$(on a spawn answerer@c --harness echo --prompt 'A test process.')" = "answerer@c" ]
on c ps answerer@c --json | has "x['state'] in ('idle', 'thinking')"
# From a, the process on c is seen by asking c.
on a ps --all --json | has "{p['id'] for p in x} == {'asker@a', 'answerer@c'}"
on a ps answerer@c | grep -q "^address    answerer@c"
echo "a created a process on c, two links away, and sees it"

step="names that must be refused"
refused on a send answerer "a bare name"
refused on a send answerer@nowhere "a kernel nobody has heard of"
refused on a send nobody@a "a process that is not there"
refused on a spawn asker "a bare name"
refused jarvis up a

step="a message there and back"
# For programs: the message's name, as JSON.
as asker@a send --json answerer@c "hello" | has "x['id'].startswith('a:') and x['to']=='answerer@c' and x['note'] is None"
answered() { on a messages asker@a --json | has "any(m['body'].startswith('ack ') and m['to']=='asker@a' for m in x)"; }
until_ok 20 answered
# The sender's kernel was told how far the message got on c.
sent_done() { on a messages asker@a --json | has "all(m['status']=='handled' for m in x if m['body']=='hello')"; }
until_ok 20 sent_done

step="following every kernel from a"
on a watch --all > "$work/watch.jsonl" 2>&1 &
watcher=$!
until_ok 20 sees "{'a','b','c'} <= {l['kernel'] for l in lines if l['kind']=='ready'}"

step="40 messages while the kernel in the middle is killed three times"
(
    for round in 1 2 3; do
        sleep 0.9
        down b KILL
        sleep 0.7
        up b
    done
) &
killer=$!
for i in $(seq 1 40); do
    as asker@a send answerer@c "question $i" > /dev/null 2>&1 < /dev/null
    sleep 0.1
done
wait $killer
all_answered() {
    [ "$(on a messages asker@a --json -n 5000 | count "m['body'].startswith('ack ') and m['to']=='asker@a' and m['status']=='handled'")" -ge 41 ]
}
until_ok 90 all_answered
on c log @c --json -n 5000 > "$work/c.json"
arrived=$(count "m['kind']=='arrive' and m['data']['body'].startswith('question ')" < "$work/c.json")
twice=$(python3 -c "
import json, sys, collections
ids = [r['data']['id'] for r in json.load(open(sys.argv[1])) if r['kind'] == 'arrive']
print(sum(1 for n in collections.Counter(ids).values() if n > 1))" "$work/c.json")
echo "questions that arrived on c: $arrived; that arrived twice: $twice"
[ "$arrived" = 40 ] && [ "$twice" = 0 ]
# Every question is known on a to have been dealt with on c.
all_known() { [ "$(on a messages asker@a --json -n 5000 | count "m['body'].startswith('question ') and m['status']=='handled'")" = 40 ]; }
until_ok 30 all_known
echo "all 40 were answered, and a knows that each was dealt with"

step="what the watcher on a received"
until_ok 30 reaches a c
# Each kernel's facts arrive once and in order, also across the outages.
seen_all() { sees "len({l['fact']['data']['id'] for l in lines if l['kind']=='fact' and l['kernel']=='c' and l['fact']['kind']=='arrive' and l['fact']['data']['body'].startswith('question ')}) == 40"; }
until_ok 40 seen_all
sees "all(ids == sorted(set(ids)) for ids in [[l['fact']['id'] for l in lines if l['kind']=='fact' and l['kernel']==k] for k in 'abc'])"
sees "any(l['kind']=='lost' for l in lines)"
echo "a followed b and c through three outages: every fact once, in order"

step="asking while a kernel cannot be reached"
down b KILL
sleep 1.5
# b was reachable a moment ago, so a still names it, and says it got nothing.
code=0
on a ps @b @c --json > "$work/ps.json" 2> "$work/ps.err" || code=$?
[ "$code" = 2 ] && grep -q "nothing from the kernel" "$work/ps.err"
on a ps --all --json | has "any(p['id']=='asker@a' for p in x)"

step="a request while the way is down"
code=0
on a stop answerer@c > "$work/stop.out" 2> "$work/stop.err" || code=$?
[ "$code" = 2 ] && grep -q "not done yet" "$work/stop.err"
up b
until_ok 60 state_of c answerer@c stopped
# Whoever asked is told what came of it.
told() { on a messages human@a --json | has "any('was carried out' in m['body'] and 'answerer@c is stopped' in m['body'] for m in x)"; }
until_ok 30 told
until_ok 30 reaches a c
[ "$(on a start answerer@c)" = "answerer@c is alive" ]
echo "a stopped a process on c while c could not be reached; it was done when the way came back"

step="a process without an agent"
[ "$(on a spawn probe@c --from probe --harness none)" = "probe@c" ]
on a send probe@c "ping one" > /dev/null
probe_answered() { on a messages human@a --json | has "any(m['from']=='probe@c' and m['body']=='probe got: ping one' for m in x)"; }
until_ok 20 probe_answered
# What its code does not take comes back to the sender as not read.
on a send probe@c "skip this" > /dev/null
came_back() { on a messages human@a --json | has "any('could not be delivered' in m['body'] and 'has no agent' in m['body'] for m in x)"; }
until_ok 20 came_back
on a messages probe@c --json | has "any(m['body']=='skip this' and m['status']=='undeliverable' for m in x)"
echo "a process with no agent answered with its code, and what it did not take came back"

step="a second way round"
# A link through a command, straight from a to c.
on a link -- jarvis link --stdio c > /dev/null
direct() { on a kernels --json | has "any(k['name']=='c' and k['hops']==1 for k in x)"; }
until_ok 20 direct
down b TERM
sleep 1
as asker@a send answerer@c "the direct way" > /dev/null
direct_answered() { on a messages asker@a --json -n 5000 | has "sum(1 for m in x if m['body'].startswith('ack ') and m['status']=='handled') >= 42"; }
until_ok 30 direct_answered
echo "with b gone, a and c talk over a link of their own"

step="a link that cannot be made says why"
on a link -- sh -c 'echo "no such host" >&2; exit 7' > /dev/null
why_shown() { on a link | grep -q "down: the command said: no such host"; }
until_ok 20 why_shown
on a log --json -n 200 | has "any(r['kind']=='link_failing' and 'no such host' in r['data']['why'] for r in x)"
on a link --remove -- sh -c 'echo "no such host" >&2; exit 7' > /dev/null

step="a kernel taken to be gone"
on a send human@b "for a kernel that is gone" > /dev/null
[ "$(on a messages --waiting --json | count "m['to']=='human@b'")" = 1 ]
on a kernels --close b > /dev/null
given_up() { on a messages human@a --json | has "any('could not be delivered' in m['body'] and 'taken to be gone' in m['body'] for m in x)"; }
until_ok 10 given_up
refused on a send human@b "after it was closed"
echo "b was taken to be gone: what waited for it was given up, and its sender told"

passed=yes
echo "PASSED"
