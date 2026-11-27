#!/usr/bin/env bash
# Run a tools/vivado-extract Tcl script under Vivado on a remote SLURM host,
# then copy its output into the local data directory.
#
#   VX_HOST       ssh host alias that has Vivado and sbatch        (required)
#   VX_SETUP      shell line that puts `vivado` on PATH there      (required)
#                 e.g. "module load <your vivado module>"
#   VX_REMOTE     remote work dir, no spaces  (default: helion-vx under remote $HOME)
#   VX_DATA       local data dir              (default: $HOME/helion-vivado-data)
#   VX_CPUS/VX_MEM/VX_TIME   sbatch resources (default 4 / 16G / 04:00:00)
#
# Usage:
#   vx.sh submit <job-name> <script.tcl> [tclargs...]   # @OUT@ in args = job's out dir
#   vx.sh status
#   vx.sh fetch  <job-name> <local-subdir>               # rsync out/ into $VX_DATA/<subdir>
#   vx.sh push                                           # copy tcl/ + py/ + rtl/ to remote
#
# Input files are pushed with `push`; reference them in tclargs as @SRC@/<path>.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
: "${VX_HOST:?set VX_HOST}"
R=${VX_REMOTE:-helion-vx}
DATA=${VX_DATA:-$HOME/helion-vivado-data}
ssh_() { ssh -o BatchMode=yes "$VX_HOST" "$@"; }

cmd=${1:?usage: vx.sh submit|status|fetch|push}; shift
case $cmd in
push)
    ssh_ "mkdir -p $R/src"
    rsync -a --delete -e "ssh -o BatchMode=yes" "$here/tcl" "$here/py" "$here/rtl" "$VX_HOST:$R/src/"
    ;;
submit)
    : "${VX_SETUP:?set VX_SETUP}"
    name=$1 script=$2; shift 2
    "$0" push
    home=$(ssh_ 'echo $HOME')
    job="$home/$R/jobs/$name"
    args=$(printf '%q ' "$@" | sed -e "s#@OUT@#$job/out#g" -e "s#@SRC@#$home/$R/src#g")
    ssh_ "mkdir -p $job/out && cat > $job/run.sbatch" <<EOF
#!/bin/bash
#SBATCH -J vx_$name
#SBATCH -N 1 -n 1 -c ${VX_CPUS:-4}
#SBATCH --mem=${VX_MEM:-16G}
#SBATCH -t ${VX_TIME:-04:00:00}
#SBATCH -o $job/slurm-%j.out
set -e
export LC_ALL=C
$VX_SETUP
cd $job
export VX_ARGS="$args"
vivado -mode batch -nojournal -log $job/vivado.log -source $home/$R/src/tcl/$script -tclargs $args
EOF
    ssh_ "sbatch $job/run.sbatch"
    ;;
status)
    ssh_ 'squeue -u $USER -o "%.10i %.20j %.8T %.10M %R"'
    ;;
fetch)
    name=$1 dest=$2
    mkdir -p "$DATA/$dest"
    rsync -a -e "ssh -o BatchMode=yes" "$VX_HOST:$R/jobs/$name/out/" "$DATA/$dest/"
    rsync -a -e "ssh -o BatchMode=yes" "$VX_HOST:$R/jobs/$name/vivado.log" "$DATA/$dest/vivado.log" || true
    echo "fetched -> $DATA/$dest"
    ;;
*) echo "unknown command $cmd" >&2; exit 2 ;;
esac
