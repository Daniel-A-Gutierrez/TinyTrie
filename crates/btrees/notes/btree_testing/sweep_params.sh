#!/bin/bash
# CTree parameter sweep: fanout (N) × preview (P)
#
# Usage: ./sweep_params.sh [--keys KEYMODE] [--corpus CORPUS] [--time SECS]
#
# For byte-key modes (random/lines/words): sweeps N × P
# For u64 modes (random-u64/seq-u64): sweeps N only (NoPreview)
#
# N values: 2, 4, 8, 12, 16
# P values: u8, u16, u32, u64 (byte keys only)

set -euo pipefail

SRCDIR="/home/d/Documents/Projects/prefix-offset-trie"
SRCFILE="$SRCDIR/src/bench/tiny_btree.rs"
RESULTS_DIR="$SRCDIR/sweep_results"

# Defaults
KEYMODE="random"
CORPUS=""
TIME="3"

while [[ $# -gt 0 ]]; do
    case $1 in
        --keys)   KEYMODE="$2"; shift 2 ;;
        --corpus) CORPUS="$2"; shift 2 ;;
        --time)   TIME="$2"; shift 2 ;;
        *) echo "Unknown option: $1"; exit 1 ;;
    esac
done

# Build bench command
BENCHCMD="cargo run --release --bin bencher -- --keys $KEYMODE --time $TIME --structures CTree"
if [ -n "$CORPUS" ]; then
    BENCHCMD="$BENCHCMD --corpus $CORPUS"
fi

# Determine if this is a u64 key mode (fixed-width, NoPreview)
IS_U64=0
if [[ "$KEYMODE" == *"u64"* ]]; then
    IS_U64=1
fi

# Fanout values to test
NS=(2 4 8 12 16)

# Preview types (byte keys only)
P_TYPES=(u8 u16 u32 u64)

# Clean and create results dir
rm -rf "$RESULTS_DIR"
mkdir -p "$RESULTS_DIR"

echo "=== CTree Parameter Sweep ==="
echo "Keys: $KEYMODE  Corpus: ${CORPUS:-none}  Time: ${TIME}s  IS_U64: $IS_U64"
echo ""

# CSV header
echo "N,P,NP1,insert_1M,lookup_1M,fwd_1M,rev_1M,mem_1M,insert_opt_1M,lookup_opt_1M,fwd_opt_1M,rev_opt_1M" > "$RESULTS_DIR/summary.csv"

if [ "$IS_U64" -eq 1 ]; then
    # Fixed-width u64: sweep N only, P=NoPreview
    echo "Mode: Fixed-width u64 (sweep N only, P=NoPreview)"
    for N in "${NS[@]}"; do
        NP1=$((N + 1))
        LABEL="N${N}_NoPreview"
        echo ""
        echo "=== Testing $LABEL (NP1=$NP1) ==="

        python3 -c "
import re
with open('$SRCFILE', 'r') as f:
    content = f.read()
content = re.sub(
    r'pub\(crate\) type CTreeFixedBench = CTreeBenchGen<u64, usize, u32, \d+, \d+, false, NoPreview>;',
    'pub(crate) type CTreeFixedBench = CTreeBenchGen<u64, usize, u32, ${N}, ${NP1}, false, NoPreview>;',
    content
)
content = re.sub(
    r'pub\(crate\) type CTreeFixedOptBench = CTreeBenchGen<u64, usize, u32, \d+, \d+, true, NoPreview>;',
    'pub(crate) type CTreeFixedOptBench = CTreeBenchGen<u64, usize, u32, ${N}, ${NP1}, true, NoPreview>;',
    content
)
with open('$SRCFILE', 'w') as f:
    f.write(content)
"

        echo "  Building..."
        if ! cargo build --release --bin bencher 2>&1 | tail -3; then
            echo "  BUILD FAILED for $LABEL - skipping"
            echo "$N,NoPreview,$NP1,BUILD_FAIL,BUILD_FAIL,BUILD_FAIL,BUILD_FAIL,BUILD_FAIL,BUILD_FAIL,BUILD_FAIL,BUILD_FAIL,BUILD_FAIL" >> "$RESULTS_DIR/summary.csv"
            continue
        fi

        echo "  Benchmarking..."
        OUTPUT=$($BENCHCMD 2>&1) || true
        echo "$OUTPUT" > "$RESULTS_DIR/${LABEL}.txt"

        INSERT_1M=$(echo "$OUTPUT" | awk '/Insertion.*keys.sec/{found=1; next} found && /^CTree[^O]/{print $NF; found=0}')
        LOOKUP_1M=$(echo "$OUTPUT" | awk '/Lookup.*keys.sec/{found=1; next} found && /^CTree[^O]/{print $NF; found=0}')
        FWD_1M=$(echo "$OUTPUT" | awk '/Iter forward.*keys.sec/{found=1; next} found && /^CTree[^O]/{print $NF; found=0}')
        REV_1M=$(echo "$OUTPUT" | awk '/Iter backward.*keys.sec/{found=1; next} found && /^CTree[^O]/{print $NF; found=0}')
        MEM_1M=$(echo "$OUTPUT" | awk '/Memory.*bytes.key/{found=1; next} found && /^CTree[^O]/{print $NF; found=0}')
        INSERT_OPT_1M=$(echo "$OUTPUT" | awk '/Insertion.*keys.sec/{found=1; next} found && /^CTreeOpt/{print $NF; found=0}')
        LOOKUP_OPT_1M=$(echo "$OUTPUT" | awk '/Lookup.*keys.sec/{found=1; next} found && /^CTreeOpt/{print $NF; found=0}')
        FWD_OPT_1M=$(echo "$OUTPUT" | awk '/Iter forward.*keys.sec/{found=1; next} found && /^CTreeOpt/{print $NF; found=0}')
        REV_OPT_1M=$(echo "$OUTPUT" | awk '/Iter backward.*keys.sec/{found=1; next} found && /^CTreeOpt/{print $NF; found=0}')

        echo "  CTree:    insert=$INSERT_1M lookup=$LOOKUP_1M fwd=$FWD_1M rev=$REV_1M mem=$MEM_1M"
        echo "  CTreeOpt: insert=$INSERT_OPT_1M lookup=$LOOKUP_OPT_1M fwd=$FWD_OPT_1M rev=$REV_OPT_1M"
        echo "$N,NoPreview,$NP1,$INSERT_1M,$LOOKUP_1M,$FWD_1M,$REV_1M,$MEM_1M,$INSERT_OPT_1M,$LOOKUP_OPT_1M,$FWD_OPT_1M,$REV_OPT_1M" >> "$RESULTS_DIR/summary.csv"
    done
else
    # Variable-length byte keys: sweep N × P
    echo "Mode: Variable-length byte keys (sweep N × P)"
    for N in "${NS[@]}"; do
        for P in "${P_TYPES[@]}"; do
            NP1=$((N + 1))
            LABEL="N${N}_P${P}"
            echo ""
            echo "=== Testing $LABEL (NP1=$NP1) ==="

            python3 -c "
import re
with open('$SRCFILE', 'r') as f:
    content = f.read()
content = re.sub(
    r'pub\(crate\) type CTreeBench = CTreeBenchGen<Vec<u8>, usize, u32, \d+, \d+, false, (u8|u16|u32|u64)>;',
    'pub(crate) type CTreeBench = CTreeBenchGen<Vec<u8>, usize, u32, ${N}, ${NP1}, false, ${P}>;',
    content
)
content = re.sub(
    r'pub\(crate\) type CTreeOptBench = CTreeBenchGen<Vec<u8>, usize, u32, \d+, \d+, true, (u8|u16|u32|u64)>;',
    'pub(crate) type CTreeOptBench = CTreeBenchGen<Vec<u8>, usize, u32, ${N}, ${NP1}, true, ${P}>;',
    content
)
with open('$SRCFILE', 'w') as f:
    f.write(content)
"

            echo "  Building..."
            if ! cargo build --release --bin bencher 2>&1 | tail -3; then
                echo "  BUILD FAILED for $LABEL - skipping"
                echo "$N,$P,$NP1,BUILD_FAIL,BUILD_FAIL,BUILD_FAIL,BUILD_FAIL,BUILD_FAIL,BUILD_FAIL,BUILD_FAIL,BUILD_FAIL,BUILD_FAIL" >> "$RESULTS_DIR/summary.csv"
                continue
            fi

            echo "  Benchmarking..."
            OUTPUT=$($BENCHCMD 2>&1) || true
            echo "$OUTPUT" > "$RESULTS_DIR/${LABEL}.txt"

            INSERT_1M=$(echo "$OUTPUT" | awk '/Insertion.*keys.sec/{found=1; next} found && /^CTree[^O]/{print $NF; found=0}')
            LOOKUP_1M=$(echo "$OUTPUT" | awk '/Lookup.*keys.sec/{found=1; next} found && /^CTree[^O]/{print $NF; found=0}')
            FWD_1M=$(echo "$OUTPUT" | awk '/Iter forward.*keys.sec/{found=1; next} found && /^CTree[^O]/{print $NF; found=0}')
            REV_1M=$(echo "$OUTPUT" | awk '/Iter backward.*keys.sec/{found=1; next} found && /^CTree[^O]/{print $NF; found=0}')
            MEM_1M=$(echo "$OUTPUT" | awk '/Memory.*bytes.key/{found=1; next} found && /^CTree[^O]/{print $NF; found=0}')
            INSERT_OPT_1M=$(echo "$OUTPUT" | awk '/Insertion.*keys.sec/{found=1; next} found && /^CTreeOpt/{print $NF; found=0}')
            LOOKUP_OPT_1M=$(echo "$OUTPUT" | awk '/Lookup.*keys.sec/{found=1; next} found && /^CTreeOpt/{print $NF; found=0}')
            FWD_OPT_1M=$(echo "$OUTPUT" | awk '/Iter forward.*keys.sec/{found=1; next} found && /^CTreeOpt/{print $NF; found=0}')
            REV_OPT_1M=$(echo "$OUTPUT" | awk '/Iter backward.*keys.sec/{found=1; next} found && /^CTreeOpt/{print $NF; found=0}')

            echo "  CTree:    insert=$INSERT_1M lookup=$LOOKUP_1M fwd=$FWD_1M rev=$REV_1M mem=$MEM_1M"
            echo "  CTreeOpt: insert=$INSERT_OPT_1M lookup=$LOOKUP_OPT_1M fwd=$FWD_OPT_1M rev=$REV_OPT_1M"
            echo "$N,$P,$NP1,$INSERT_1M,$LOOKUP_1M,$FWD_1M,$REV_1M,$MEM_1M,$INSERT_OPT_1M,$LOOKUP_OPT_1M,$FWD_OPT_1M,$REV_OPT_1M" >> "$RESULTS_DIR/summary.csv"
        done
    done
fi

echo ""
echo "=== Sweep complete. Results in $RESULTS_DIR/summary.csv ==="