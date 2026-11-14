#!/bin/bash
# Build check of the non-default Cargo feature sets (Cargo.toml), so none of
# them rots unnoticed:
#   no-ad2-isr        --no-default-features --features vdisk (no interrupt)
#   spi-disk          --no-default-features --features ad2-isr (the writable
#                     SPI disk as the USB LUN, no generated volume)
#   fatren-dry-run, keep-screen
#   (acdc-meth, the AC+DC prototype methods B / C, src/acdcm.rs: not built
#   in this fork, below)
#   (memprobe, DBG:PEEK? / POKE / MTEST?, src/memprobe.rs: not built in
#   this fork, below)
# Each one is a full build.sh run (link asserts: TCM stack, SRAM, regions
# A2 / B / C; stackdepth.py --check 20480; pack + whole 128-byte blocks) into
# its own DM40_TDIR / DM40_OUT under target/variants/, so firmware/target/native
# and opendm40.bin (the image tools/reload.sh installs) are never
# replaced. Not run on any device. One line per variant, then PASS / FAIL.
# ~1-2 min per variant (a full release link each).
set -u
cd "$(dirname "$0")/../.."
V=target/variants
mkdir -p "$V"
fail=0
run() { # name, build.sh args...
    local n=$1; shift
    if DM40_TDIR=$V/$n DM40_OUT=$V/$n/opendm40-$n.bin ./build.sh "$@" > "$V/$n.log" 2>&1; then
        echo "ok   $n: $(grep -m1 '^stack:' "$V/$n.log" | sed 's/.*headroom/stack headroom/'); $(grep -m1 'region B free' "$V/$n.log" | sed 's/ *(.*//; s/^ *//; s/  *= */ /'); $(grep -m1 'region C free' "$V/$n.log" | sed 's/ *(.*//; s/^ *//; s/  *= */ /')"
    else
        echo "FAIL $n (build.sh $*): $V/$n.log"
        grep -E '^error|error:|assert|Error' "$V/$n.log" | head -5 | sed 's/^/     /'
        fail=1
    fi
}
run no-ad2-isr --no-default-features --features vdisk
run spi-disk --no-default-features --features ad2-isr
run fatren-dry-run --features fatren-dry-run
run keep-screen --features keep-screen
# acdc-meth (the AC+DC prototype methods, never in a shipped image) is not
# built in this fork: its region B fell under the 8 KiB link floor with the
# DMM trigger source and CH1 Cur / CH2 V / A (~0.8 KiB short); build it by
# hand when working on it.
# memprobe (DBG:PEEK? / POKE / MTEST?, a developer's tool, never in a
# shipped image) is not built in this fork: its region C was 16 B above the
# 4 KiB floor at 0.4.19-S2084 and the DPO / wide-graph / diode-band code
# took it under; build it by hand when needed (with features dropped).
[ -f opendm40.bin ] && echo "opendm40.bin untouched: $(stat -c %y opendm40.bin)"
if [ $fail = 0 ]; then echo PASS; else echo FAIL; exit 1; fi
