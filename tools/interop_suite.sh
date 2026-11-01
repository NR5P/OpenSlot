#!/usr/bin/env bash
# Feature-by-feature interop suite: Openslot (examples/interop_peer) against a real
# VarAC station, judged by what VarAC itself writes to its traffic log.
#
#   tools/interop_suite.sh VARAC_TRAFFIC_LOG [PEER_PORT] [KISS_PORT] [BW] [FILE_DIR]
#
# VarAC must be running as NR5P (BBS enabled with bbs_test.txt, parking allowed) and
# reachable over the air from the modem on PEER_PORT. Prints PASS/FAIL per feature.
set -uo pipefail
LOG=$1 PORT=${2:-8400} KISS=${3:-8200} BW=${4:-500} DIR=${5:-/tmp}
PEER=(target/debug/examples/interop_peer --port "$PORT" --bw "$BW")
pass=0 fail=0

printf 'Hello VarAC, this file came from Openslot.\n' > "$DIR/suite_file.txt"
printf 'Attachment from Openslot.\n' > "$DIR/suite_attach.txt"
mkdir -p "$DIR/suite_rx"

# run NAME EXPECTED_REGEX -- peer args...
run() {
    local name=$1 want=$2; shift 3
    local before; before=$(wc -l < "$LOG")
    timeout 330 "${PEER[@]}" "$@" > "$DIR/suite_$name.log" 2>&1
    sleep 8
    if tail -n +$((before + 1)) "$LOG" | grep -Eq "$want"; then
        echo "PASS  $name"; pass=$((pass + 1))
    else
        echo "FAIL  $name   (wanted /$want/ in VarAC traffic log; peer log: $DIR/suite_$name.log)"; fail=$((fail + 1))
    fi
    sleep 15   # let VarAC and the modems settle between links
}

run connect_chat   'W5TST> suite chat'                 -- --call W5TST --connect NR5P --qsy -75 --chat "suite chat" --secs 110
run requests       'NR5P> VarAC V'                     -- --call W5TST --connect NR5P --qsy -75 --gap 15 --chat "<INFO>" --chat "<SNRR>" --chat "<VER>" --secs 160
run ping           'INCOMING PING'                     -- --call W5TST --connect NR5P-T --secs 60
run vmail          'NR5P> <SMR>'                       -- --call W5TST --connect NR5P --qsy -75 --vmail "Suite VMail|Plain VMail from Openslot" --secs 140
run vmail_attach   'RECEIVING VMAIL WITH ATTACHMENT'   -- --call W5TST --connect NR5P --qsy -75 --vmail-attach "$DIR/suite_attach.txt" --secs 200
run file_send      'FILE SUCCESSFULLY RECEIVED'        -- --call W5TST --connect NR5P --qsy -75 --send-file "$DIR/suite_file.txt" --secs 200
run bbs            'FILE SUCCESSFULLY SENT'            -- --call W5TST --connect NR5P --qsy -75 --gap 30 --chat "<BLR>" --chat "<BG:bbs_test.txt>" --save-dir "$DIR/suite_rx" --secs 200
run park           'NR5P> <SMR>'                       -- --call W5TST --connect NR5P --qsy -75 --park-for K5XYZ --secs 140
sleep 120   # original VarAC didn't hand over mail parked ~2 min earlier in one run; give it time
run relay_collect  'VMAIL DELIVERED SUCCESSFULLY'      -- --call K5XYZ --connect NR5P --qsy -75 --secs 160
run broadcast      'BROADCAST .*FROM: W5TST'           -- --call W5TST --kiss-port "$KISS" --broadcast "suite broadcast" --secs 45

echo "-----"
echo "passed $pass, failed $fail"
