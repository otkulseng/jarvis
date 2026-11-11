#!/bin/sh
# A network of three kernels on this machine, and what must hold in it:
#
#   - a message crosses kernels that are not linked directly, and a longer
#     way round is used when a link is taken away;
#   - while the kernel in the middle is killed again and again, every message
#     still arrives, and arrives once;
#   - what happened can be read from the store alone, before and after the
#     store is compacted.
#
# Run it from anywhere: sh tests/network.sh. It uses its own repository,
# store and sockets under /tmp, and no agent (tests/harness/echo answers).
set -eu
here=$(cd "$(dirname "$0")/.." && pwd)
cargo build --quiet --manifest-path "$here/Cargo.toml"

# Short, because a socket's path must be.
work=$(mktemp -d /tmp/jv-XXXXXX)
export JARVIS_RUN="$work/run" PATH="$here/target/debug:$PATH"
unset JARVIS_KERNEL JARVIS_ID JARVIS_STORE || true
mkdir -p "$work/run" "$work/repo/harness"
cp "$here/tests/harness/echo" "$work/repo/harness/echo"
cd "$work/repo"
git init -q .
git add -A
git -c user.name=test -c user.email=test@example.com commit -q -m "a repository for the test"

up() { # up NAME: start a kernel in the background
    jarvis up "$1" --ship-every 0.5 --scan-every 0.5 >> "$work/$1.log" 2>&1 &
    echo $! > "$work/$1.pid"
}
down() { kill "-$2" "$(cat "$work/$1.pid")" 2>/dev/null || true; }
finish() {
    for k in a b c; do down $k TERM; done
    sleep 1
    [ "${passed:-no}" = yes ] && rm -rf "$work" || echo "FAILED. What was left is in $work" >&2
}
trap finish EXIT
# until SECONDS COMMAND...: wait until the command succeeds
until_ok() {
    limit=$1; shift
    i=0
    while ! "$@" > /dev/null 2>&1; do
        i=$((i + 1))
        [ $i -gt $((limit * 5)) ] && { echo "gave up waiting for: $*" >&2; exit 1; }
        sleep 0.2
    done
}
count() { python3 -c "import json,sys; print(len([m for m in json.load(sys.stdin) if $1]))"; }

for k in a b c; do up $k; done
for k in a b c; do until_ok 10 jarvis --on $k kernels; done

jarvis --on a spawn asker --harness echo --prompt "A test process." > /dev/null
jarvis --on c spawn answerer --harness echo --prompt "A test process." > /dev/null
jarvis --on a link b > /dev/null
jarvis --on b link c > /dev/null
reaches_c() { jarvis --on a kernels --json | python3 -c "import json,sys; sys.exit(0 if any(k['name']=='c' and k['reachable'] for k in json.load(sys.stdin)) else 1)"; }
until_ok 20 reaches_c
echo "three kernels, linked a - b - c"

# The kernel in the middle dies three times while 40 messages are sent through it.
(
    for round in 1 2 3; do
        sleep 1
        down b KILL
        sleep 0.7
        up b
    done
) &
killer=$!
for i in $(seq 1 40); do
    JARVIS_KERNEL=a JARVIS_ID=asker@a jarvis send answerer@c "question $i" > /dev/null 2>&1 < /dev/null
    sleep 0.1
done
wait $killer
echo "40 messages sent from a to c while b was killed three times"

all_answered() {
    [ "$(jarvis --on a messages --json -n 5000 | count "m['body'].startswith('ack ') and m['to']=='asker@a' and m['status']=='handled'")" -ge 40 ]
}
until_ok 90 all_answered
sleep 1.5
messages=$(jarvis --on a messages --json -n 5000)
questions=$(echo "$messages" | count "m['body'].startswith('question ') and m['status']=='handled'")
answers=$(echo "$messages" | count "m['body'].startswith('ack ') and m['to']=='asker@a'")
twice=$(jarvis --on c log --json -n 100000 | python3 -c "
import json, sys, collections
arrived = collections.Counter(f['data']['id'] for f in json.load(sys.stdin) if f['kind'] == 'arrive' and f['kernel'] == 'c')
print(len([m for m, times in arrived.items() if times > 1]))")
echo "questions dealt with: $questions; answers back: $answers; messages that arrived twice: $twice"
[ "$questions" = 40 ] && [ "$answers" = 40 ] && [ "$twice" = 0 ]

# A link through a command gives a second way round (a - c). Taking the
# direct link a - b away then leaves b reachable, through c.
kernel_is() { jarvis --on a kernels --json | python3 -c "import json,sys; sys.exit(0 if any(k['name']=='$1' and ($2) for k in json.load(sys.stdin)) else 1)"; }
jarvis --on a link -- jarvis --on c link --stdio > /dev/null
until_ok 20 kernel_is c "k['hops']==1"
jarvis --on a link --remove b > /dev/null
until_ok 20 kernel_is b "k['reachable'] and k['via']=='c'"
JARVIS_KERNEL=a jarvis spawn far --on b --harness echo --prompt "A test process." > /dev/null
until_ok 20 kernel_is b "k['processes']==1"
echo "a second way round took over when a link was taken away, and a process was created two kernels away"

# A bare name is this kernel's process only: `answerer` runs on c, not on a.
if JARVIS_KERNEL=a jarvis send answerer "to the wrong place" > /dev/null 2>&1 < /dev/null; then
    echo "a bare name reached a process on another kernel" >&2; exit 1
fi

# A second kernel of a name that is taken is refused.
if jarvis up a > /dev/null 2>&1; then echo "a second kernel called a was allowed" >&2; exit 1; fi

# The store alone tells the same story, before and after it is compacted.
for k in a b c; do down $k TERM; done
sleep 1.5
store="$work/repo/.jarvis/store"
before=$(jarvis messages --store "$store" --json -n 5000 | count "True")
jarvis store compact --store "$store" > /dev/null
[ "$(find "$store" -name '*.jsonl' | wc -l | tr -d ' ')" = 0 ]
after=$(jarvis messages --store "$store" --json -n 5000 | count "True")
echo "messages in the store: $before; after compacting it into one file: $after"
[ "$before" = "$after" ] && [ "$before" -ge 80 ]

passed=yes
echo "PASSED"
