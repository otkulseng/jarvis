#!/bin/sh
# Many kernels on this machine, linked as a tree (each has up to three below
# it), as an allocation divided into parts would be. What must hold:
#
#   - from one kernel, a process is created on every other, and all are seen;
#   - messages between processes all over the tree each arrive once, while
#     kernels inside the tree are killed and started again;
#   - a kernel that nothing is sent to does nothing: it writes no records and
#     uses next to no processor time, however many kernels there are.
#
# sh tests/many.sh [COUNT]   COUNT kernels, 40 by default. No agent is run.
set -eu

count=${1:-40}
here=$(cd "$(dirname "$0")/.." && pwd)
cargo build --quiet --manifest-path "$here/Cargo.toml"

work=$(mktemp -d /tmp/jv-XXXXXX)
export JARVIS_RUN="$work/run" PATH="$here/target/debug:$PATH"
unset JARVIS_KERNEL JARVIS_ID || true
mkdir -p "$work/run" "$work/repo/harness"
cp "$here/tests/harness/echo" "$work/repo/harness/echo"
cd "$work/repo"
git init -q -b main .
git add -A
git -c user.name=test -c user.email=test@example.com commit -q -m "a repository for the test"

on() { k=$1; shift; JARVIS_KERNEL=$k jarvis "$@"; }
as() { who=$1; shift; JARVIS_KERNEL=${who#*@} JARVIS_ID=$who jarvis "$@"; }
up() { jarvis up "$1" >> "$work/$1.log" 2>&1 & echo $! > "$work/$1.pid"; }
down() { kill "-$2" "$(cat "$work/$1.pid")" 2>/dev/null || true; }
finish() {
    for i in $(seq 0 $((count - 1))); do down "k$i" TERM; done
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
# The processor time all kernels have used so far, in seconds.
cpu() {
    ps -o time= -p "$(cat "$work"/k*.pid | tr '\n' ',' | sed 's/,$//')" | python3 -c "
import sys
total = 0.0
for line in sys.stdin:
    parts = [float(p) for p in line.strip().replace('-', ':').split(':')]
    total += sum(p * 60 ** i for i, p in enumerate(reversed(parts)))
print(round(total, 2))"
}

step="starting $count kernels as a tree"
for i in $(seq 0 $((count - 1))); do up "k$i"; done
for i in $(seq 0 $((count - 1))); do until_ok 20 on "k$i" kernels; done
for i in $(seq 1 $((count - 1))); do on "k$i" link "k$(((i - 1) / 3))" > /dev/null; done
all_reached() { on k0 kernels --json | has "sum(1 for k in x if k['reachable']) == $count"; }
until_ok 60 all_reached
far=$(on k0 kernels --json | python3 -c "import json,sys; print(max(k['hops'] for k in json.load(sys.stdin)))")
echo "$count kernels are up; from k0 the farthest is $far links away"

step="creating a process on every kernel, from k0"
for i in $(seq 0 $((count - 1))); do on k0 spawn "w@k$i" --harness echo --prompt "A test process." > /dev/null; done
began=$(python3 -c "import time; print(time.time())")
on k0 ps --all --json > "$work/ps.json"
took=$(python3 -c "import time; print(round(time.time() - $began, 2))")
has "len(x) == $count" < "$work/ps.json"
echo "k0 created $count processes and sees them all; asking every kernel took ${took}s"

step="messages all over the tree while kernels inside it are killed"
(
    for victim in k1 k2 k1; do
        sleep 1.5
        down $victim KILL
        sleep 1
        up $victim
    done
) &
killer=$!
sent=0
for round in 1 2 3; do
    for i in $(seq 0 $((count - 1))); do
        to=$(((i * 7 + 3 + round) % count))
        [ "$to" = "$i" ] && continue
        as "w@k$i" send "w@k$to" "question $round from $i" > /dev/null 2>&1 < /dev/null
        sent=$((sent + 1))
    done
done
wait $killer
until_ok 60 all_reached
none_waiting() { on k0 messages --all --waiting --json -n 100000 | has "len(x) == 0"; }
until_ok 120 none_waiting
on k0 log --all --json -n 1000000 > "$work/log.json"
python3 - "$work/log.json" "$sent" <<'EOF'
import collections, json, sys
facts = json.load(open(sys.argv[1]))
arrived = [f["data"]["id"] for f in facts if f["kind"] == "arrive" and f["data"]["body"].startswith("question ")]
local = [f["data"]["id"] for f in facts if f["kind"] == "send" and f["data"]["body"].startswith("question ") and f["data"]["to"].split("@")[1] == f["kernel"]]
twice = sum(1 for n in collections.Counter(arrived).values() if n > 1)
print(f"questions sent: {sys.argv[2]}; arrived: {len(arrived) + len(local)}; arrived twice: {twice}")
sys.exit(0 if len(arrived) + len(local) == int(sys.argv[2]) and twice == 0 else 1)
EOF

step="what a kernel costs when nothing is sent to it"
sleep 2
records() { on "$1" log --json -n 1000000 | python3 -c "import json,sys; print(len(json.load(sys.stdin)))"; }
before=$(records "k$((count - 1))")
cpu_before=$(cpu)
sleep 10
after=$(records "k$((count - 1))")
cpu_after=$(cpu)
echo "in 10 quiet seconds, the last kernel wrote $((after - before)) records, and all $count kernels together used $(python3 -c "print(round($cpu_after - $cpu_before, 2))")s of processor time"
[ "$after" = "$before" ]

passed=yes
echo "PASSED"
