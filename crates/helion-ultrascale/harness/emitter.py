# Emitter discovery and protocol (bitgen-core owns the emitter).
#
# The harness never invents an emitter. It looks for one in exactly
# these places, in order:
#   1. --emitter-cmd on the diff.py command line
#   2. HELION_BITGEN_EMITTER environment variable
# If neither is set, there is no emitter and every block that needs one
# reports blocked with the missing piece named, never zero.
#
# Emitter protocol (for the bitgen-core / integration worker):
#   <emitter> emit --fasm FILE --part PART --out FILE.bit
#       writes a bitstream for the FASM design, exit 0 on success.
#   <emitter> parse --bit FILE.bit --part PART
#       prints the FASM the bitstream encodes, on stdout, exit 0.
#   <emitter> bits --bit FILE.bit --part PART   (optional)
#       prints the emitted bitset, one line per set bit:
#       FAR_HEX OFFSET VALUE BLOCK
#       where FAR_HEX is the 32-bit frame address in hex, OFFSET is
#       the bit offset within the 93-word frame, VALUE is 0 or 1, and
#       BLOCK is the block-registry name the bit belongs to (the
#       emitter knows which FASM feature produced each bit). Block
#       labels are what let the harness compare per block: without
#       them, one block's set bits would count as diffs in another
#       block. If the emitter does not implement `bits`, or prints
#       lines without a BLOCK label, the harness derives the emitted
#       bitset through `parse` instead and says so in the record
#       (parse-mediated, weaker: it cannot catch a bug shared by emit
#       and parse).
#
# No em dashes anywhere.

import os
import subprocess

NO_EMITTER_MISSING = (
    "bitgen-core emitter absent: no --emitter-cmd and HELION_BITGEN_EMITTER "
    "unset; emit_bitstream/parse_bitstream not present on branch "
    "fm/ultra-bitgen-core yet"
)


class EmitterError(Exception):
    pass


class Emitter:
    def __init__(self, cmd):
        self.cmd = cmd

    def _run(self, args, timeout=300):
        try:
            proc = subprocess.run(
                [self.cmd] + args,
                capture_output=True, text=True, timeout=timeout,
            )
        except FileNotFoundError:
            raise EmitterError("emitter command not found: %s" % self.cmd)
        except subprocess.TimeoutExpired:
            raise EmitterError("emitter timed out after %ds: %s" % (timeout, args))
        if proc.returncode != 0:
            raise EmitterError(
                "emitter %s failed (exit %d): %s"
                % (" ".join(args), proc.returncode, proc.stderr.strip()[:500])
            )
        return proc.stdout

    def emit(self, fasm_path, part, out_path):
        self._run(["emit", "--fasm", fasm_path, "--part", part, "--out", out_path])

    def parse(self, bit_path, part):
        return self._run(["parse", "--bit", bit_path, "--part", part])

    def bits(self, bit_path, part):
        """Emitted bitset from the bitstream bytes, per block.

        Returns {block: {(far, offset): value}} or None when the
        emitter does not implement `bits` or prints lines without a
        BLOCK label (caller falls back to parse-mediated derivation).
        """
        try:
            out = self._run(["bits", "--bit", bit_path, "--part", part])
        except EmitterError as e:
            if "unknown" in str(e).lower() or "unrecognized" in str(e).lower():
                return None
            raise
        per_block = {}
        for line in out.splitlines():
            line = line.strip()
            if not line:
                continue
            fields = line.split()
            if len(fields) != 4:
                return None
            far_s, off_s, val_s, block = fields
            per_block.setdefault(block, {})[(int(far_s, 16), int(off_s))] = int(val_s)
        return per_block

    def describe(self):
        return "emitter command: %s" % self.cmd


def find_emitter(explicit_cmd=None):
    """Return an Emitter, or None when no emitter is configured."""
    cmd = explicit_cmd or os.environ.get("HELION_BITGEN_EMITTER")
    if not cmd:
        return None
    return Emitter(cmd)
