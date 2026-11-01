#!/usr/bin/env bash
# Interop lab: two XARA modems (VARA HF compatible) linked over PipeWire null sinks,
# each behind a logging tap (tools/vara_tap.py).
#
#   station A: clients connect to cmd 8300 / data 8301 / KISS 8100  (tap inject: 8399)
#   station B: clients connect to cmd 8400 / data 8401 / KISS 8200  (tap inject: 8499)
#
#   tools/interop_lab.sh up   LABDIR XARA_BIN CALL_A CALL_B
#   tools/interop_lab.sh down LABDIR
#
# XARA_BIN must be built with the pulse feature. All traffic is logged to LABDIR/tap.log.
set -euo pipefail
cmd=$1 lab=$2
here=$(cd "$(dirname "$0")" && pwd)

if [ "$cmd" = up ]; then
    xara=$3 call_a=$4 call_b=$5
    mkdir -p "$lab"
    for n in xara_a2b xara_b2a; do
        pw-cli create-node adapter "{ factory.name=support.null-audio-sink node.name=$n node.description=$n media.class=Audio/Sink object.linger=true audio.position=[MONO] audio.rate=48000 }" >/dev/null
    done
    sleep 1
    pw-cli ls Node | awk '/^\tid/{id=$2} /node.name = "xara_/{print id}' | tr -d ',' > "$lab/nodes.txt"
    XDG_CONFIG_HOME=$lab/cfgA setsid "$xara" --call "$call_a" --hf 2300 --audio pulse --playback xara_a2b --capture xara_b2a.monitor --port 9300 --kiss-port 9100 > "$lab/xaraA.log" 2>&1 &
    XDG_CONFIG_HOME=$lab/cfgB setsid "$xara" --call "$call_b" --hf 2300 --audio pulse --playback xara_b2a --capture xara_a2b.monitor --port 9400 --kiss-port 9200 > "$lab/xaraB.log" 2>&1 &
    sleep 2
    setsid python3 "$here/vara_tap.py" --name A --listen 8300 --kiss-listen 8100 --modem 9300 --kiss-modem 9100 --inject 8399 --log "$lab/tap.log" > "$lab/tapA.err" 2>&1 &
    setsid python3 "$here/vara_tap.py" --name B --listen 8400 --kiss-listen 8200 --modem 9400 --kiss-modem 9200 --inject 8499 --log "$lab/tap.log" > "$lab/tapB.err" 2>&1 &
    sleep 1
    echo "lab up in $lab"
elif [ "$cmd" = down ]; then
    pkill -f "vara_tap.py --name [AB]" || true
    pkill -f -- "--audio pulse --playback xara_" || true
    if [ -f "$lab/nodes.txt" ]; then
        while read -r id; do pw-cli destroy "$id" >/dev/null 2>&1 || true; done < "$lab/nodes.txt"
    fi
    echo "lab down"
fi
