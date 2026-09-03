# Security

## Supported versions

Report issues against **`master`**. There is no LTS branch yet.

## What to report privately

- Remote code execution in `helion-ide`, Tcl ingest, or bitstream decode
- Path traversal when opening sources / HAD
- Secrets in logs or the repo

Open a [private GitHub security advisory](https://github.com/helion-fpga/helion/security/advisories/new) or contact **@saksham-45**.

## What is not a vulnerability

- QoR / WNS changes, missing Vivado Tcl, or “does not fit Ibex on HL10T-C32-1”
- Legal-fence refusals (UNISIM, vendor backends)

Do not attach AMD/Xilinx bitstreams or encrypted IP.
