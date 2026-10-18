# Vivado Benchmark on Mac (OrbStack) - Full Instructions

Goal: run Vivado synthesis on 6 benchmark designs inside your x86 OrbStack
container, and collect LUT/FF/DSP/BRAM counts to compare against Helion.

No VPN. No cluster. Everything runs on your Mac.

---

## Prerequisites

- OrbStack installed and running
- An x86 container with Vivado installed
- The helion repo cloned on your Mac
- `docker` CLI working in Terminal (`docker ps` shows your container)

---

## Step 1: Update the repo

Open Terminal on your Mac:

```bash
cd ~/workspace/helion-ide-merge
git pull origin wip/ide-integration
```

If your repo lives somewhere else, `cd` there instead and use that path
in place of `~/workspace/helion-ide-merge` in the steps below.

Verify the benchmark files exist:

```bash
ls vivado_bench.tcl examples/blinky.sv third_party/picorv32/picorv32.v
```

All three should list without errors.

---

## Step 2: Find your Vivado container

```bash
docker ps --format '{{.Names}}'
```

Pick the container that has Vivado. In the steps below, replace
`<container>` with its name.

Sanity check that Vivado is reachable inside it:

```bash
docker exec <container> bash -c 'which vivado || ls /opt/Xilinx/Vivado/'
```

You should see a path to `vivado` or a version directory. If neither,
Vivado isn't installed in that container, try another one.

---

## Step 3: Copy benchmark files into the container

```bash
REPO=~/workspace/helion-ide-merge
C=<container>

docker cp $REPO/vivado_bench.tcl $C:/tmp/
docker cp $REPO/examples/blinky.sv $C:/tmp/
docker cp $REPO/examples/counter.sv $C:/tmp/
docker cp $REPO/examples/hier.sv $C:/tmp/
docker cp $REPO/examples/complex.sv $C:/tmp/
docker cp $REPO/examples/serv_pin_wrap.sv $C:/tmp/
docker cp $REPO/third_party/picorv32/picorv32.v $C:/tmp/
```

---

## Step 4: Run the Vivado benchmark

```bash
docker exec -it $C bash -c 'cd /tmp && source /opt/Xilinx/Vivado/*/settings64.sh 2>/dev/null; vivado -mode batch -source vivado_bench.tcl 2>&1 | tee vivado_run.log'
```

What this does:
- Sources Vivado's environment (skipped silently if already on PATH)
- Runs synthesis for all 6 designs against part `xc7a35tcpg236-1` (Arty A7)
- Saves the full log to `/tmp/vivado_run.log` inside the container

Expected runtime: a few minutes (picorv32 is the slow one).

Watch for lines like:

```
=== Running picorv32 (top=picorv32) ===
picorv32: luts=1234 ffs=567 dsps=0 brams=0
```

---

## Step 5: Copy results back to your Mac

```bash
docker cp $C:/tmp/bench_results_vivado.csv $REPO/
cat $REPO/bench_results_vivado.csv
```

You should see:

```
design,luts,ffs,dsps,brams
blinky,...
counter,...
hier,...
complex,...
picorv32,...
serv,...
```

---

## Step 6: Send the results back

Paste the full contents of `bench_results_vivado.csv` to zuck.
He will build the Helion vs Vivado comparison table.

Helion's numbers for reference (already collected):

```
design,cells,luts,ffs,dsps,brams,runtime_s
blinky,3,1,0,0,0,0
counter,9,4,0,0,0,0
hier,3,1,0,0,0,0
complex,158,71,0,0,0,0
picorv32,3030,1697,0,0,0,16
serv,25,15,0,0,0,0
```

---

## Troubleshooting

**`docker: command not found`**
- Open OrbStack, make sure it's running. The docker CLI ships with it.
- Try `/opt/orbstack/bin/docker ps` if `docker` isn't on your PATH.

**Wrong container picked**
- `docker ps` lists all running containers. Re-run Step 2 and pick
  the one with Vivado.

**`vivado: command not found` inside the container**
- Find the install: `docker exec <container> ls /opt/Xilinx/Vivado/`
- Source it manually: `docker exec -it <container> bash -c 'source /opt/Xilinx/Vivado/2024.1/settings64.sh && which vivado'`
- Adjust the version number to match your install.

**TCL script errors on a design**
- Check `/tmp/vivado_run.log` inside the container:
  `docker exec <container> cat /tmp/vivado_run.log | tail -50`
- Common cause: top module name mismatch. The script uses
  blinky, counter, hier, complex, picorv32, serv_pin_wrap.
- Send the error to zuck and he'll fix the script.

**Synthesis takes very long**
- picorv32 is the heavy one. Give it 10+ minutes.
- The other five should finish in under a minute each.

**Out of disk in the container**
- The script deletes each project dir after synthesis (`file delete -force`),
  so disk use stays bounded. If it still fills up, prune with
  `docker exec <container> rm -rf /tmp/vivado_bench_*`.
