#!/bin/bash
# Vivado 2017.3 builds of the rung 4 design and the CMT sweep (USF cluster,
# SLURM partition sc). Usage: bash cmt_sweep_jobs.sh [ref_mmcm | sweep_<name> ...]
cd "$(dirname "$0")"; D=$PWD
V=/apps/xilinx/vivado/2017.3/Vivado/2017.3/settings64.sh
P=xc7z020clg484-1
sub() { mkdir -p out/$1; sbatch -p sc -J hm_$1 -o out/$1/slurm.log -c 2 --mem=${MEM:-6G} --wrap "source $V >/dev/null; cd $D/out/$1; $2"; }
R=$D/rtl   # zedboard_mmcm.{sv,xdc} from examples/, sweep_* from cmt_sweep_gen.py
for j in ${@:-ref_mmcm}; do case $j in
  ref_mmcm) sub $j "vivado -mode batch -nojournal -log vivado.log -source $D/ref_build.tcl -tclargs . zedboard_mmcm $P $R/zedboard_mmcm.xdc $R/zedboard_mmcm.sv" ;;
  sweep_*)  n=${j#sweep_}; sub $j "vivado -mode batch -nojournal -log vivado.log -source $D/ref_build.tcl -tclargs . sweep_$n $P $R/sweep_$n.xdc $R/sweep_$n.sv" ;;
esac; done
squeue -u $USER
