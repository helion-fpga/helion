"""Shared test setup: one Db and one Fabric per module, loaded once.
"""
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from us09v.db import Db, default_db_root  # noqa: E402
from us09v.fabric import Fabric  # noqa: E402

DB_ROOT = default_db_root()
FIXTURES = os.path.join(
    os.path.dirname(os.path.abspath(__file__)), "fixtures"
)


def load_db():
    return Db(DB_ROOT)


def load_fabric(db):
    return Fabric(db)


def load_fixture(name):
    """(fasm path, rtl path, port map dict) for a fixture."""
    d = os.path.join(FIXTURES, name)
    with open(os.path.join(d, "port_map.json")) as f:
        port_map = json.load(f)
    return os.path.join(d, f"{name}.fasm"), os.path.join(d, "rtl.v"), port_map


def fixture_ports(name):
    """(port_ins, port_outs, cosim pin_of) for a fixture, from its
    RTL port directions and its port map."""
    from us09v.cosim import sv_ports
    from us09v.ports import explicit_ports_in_out

    fasm_path, rtl_path, port_map = load_fixture(name)
    ports = sv_ports([rtl_path], _rtl_top(rtl_path))
    in_bits = {b for p in ports if p.direction == "in" for b in p.bits()}
    out_bits = {b for p in ports if p.direction == "out" for b in p.bits()}
    return explicit_ports_in_out(port_map, in_bits, out_bits)


def _rtl_top(rtl_path):
    import re

    with open(rtl_path) as f:
        text = f.read()
    m = re.search(r"module\s+(\w+)", text)
    return m.group(1)


def byp_shape_tiles():
    """The BYP-shape blinky tiles: the old flow FASM (13 lines, 6
    PIPs, LUT O routed through the INT to the AX pin) plus
    SLICE_X1Y0.FFMUXA1.BYP. Fresh dict on every call so tests can
    mutate their copy. Route must pass on this shape: the LUT O
    pin A_O drives the AX leg, the FF Q pin AQ drives the A1 leg.
    """
    return {
        "SLICE_X1Y0": {
            "A6LUT.INIT[63:0]": 0x5555555555555555,
            "AFF.INIT.V0": None,
            "AFF.SRVAL.V0": None,
            "ABCDFF.CEUSED.V0": None,
            "ABCDFF.SRUSED.V0": None,
            "ABCDFF.CLKINV.V0": None,
            "ABCDFF.SRINV.V0": None,
            "FFMUXA1.BYP": None,
        },
        "INT_X1Y0": {
            "PIP.INT_NODE_IMUX_42_INT_OUT1.LOGIC_OUTS_W14": None,
            "PIP.BYPASS_W5.INT_NODE_IMUX_42_INT_OUT1": None,
            "PIP.INT_NODE_IMUX_48_INT_OUT0.BYPASS_W5": None,
            "PIP.IMUX_W10.INT_NODE_IMUX_48_INT_OUT0": None,
            "PIP.INODE_W_1_FT1.LOGIC_OUTS_W0": None,
            "PIP.BOUNCE_W_0_FT1.INODE_W_1_FT1": None,
        },
    }


def byp_shape_port_ins():
    """Primary inputs for the BYP shape: the clock on CLK1."""
    return {"clk": ("SLICE_X1Y0", "CLK1")}


def clock_leg_tiles():
    """The clock-leg shape (spanclk lane): the BYP-shape entry slice
    plus a second slice SLICE_X4Y0 whose ABCD FF clocks through a
    GCLK leg on INT_X3Y0 (CTRL_W4 <- GLOBAL_14 <- GCLK_B_0_9).
    The second slice's LUT B is constant, so it has no LUT-input
    sink; its only fabric sink is CLK1, driven by the leg. Fresh
    dict on every call so tests can mutate their copy.
    """
    tiles = byp_shape_tiles()
    tiles["SLICE_X4Y0"] = {
        "B6LUT.INIT[63:0]": 0xFFFFFFFFFFFFFFFF,
        "BFF.INIT.V0": None,
        "BFF.SRVAL.V0": None,
        "ABCDFF.CEUSED.V0": None,
        "ABCDFF.SRUSED.V0": None,
        "ABCDFF.CLKINV.V0": None,
        "ABCDFF.SRINV.V0": None,
        "FFMUXB1.D6": None,
    }
    tiles["INT_X3Y0"] = {
        "PIP.CTRL_W4.INT_NODE_GLOBAL_14_INT_OUT0": None,
        "PIP.INT_NODE_GLOBAL_14_INT_OUT0.GCLK_B_0_9": None,
    }
    return tiles


def byp_cross_letter_tiles():
    """The cross-letter BYP shape (bypleg lane): the byp2 flow
    design, reduced to the two routed legs. LUT A drives FF A1
    through D6 (AQ feeds the A LUT input back); LUT B drives FF
    A2 through the fabric (FFMUXA2.BYP): B_O on LOGIC_OUTS_W12
    through IMUX_40, BYPASS_W6, INODE_W_9 to BOUNCE_W_0 (the AX
    pin). B INIT is constant, so B has no LUT-input sink; the A
    leg keeps the A1 sink driven. Fresh dict on every call so
    tests can mutate their copy. Route must pass on this shape.
    """
    return {
        "SLICE_X1Y0": {
            "A6LUT.INIT[63:0]": 0x5555555555555555,
            "B6LUT.INIT[63:0]": 0xFFFFFFFFFFFFFFFF,
            "AFF.INIT.V0": None,
            "AFF.SRVAL.V0": None,
            "AFF2.INIT.V0": None,
            "AFF2.SRVAL.V0": None,
            "ABCDFF.CEUSED.V0": None,
            "ABCDFF.SRUSED.V0": None,
            "ABCDFF.CLKINV.V0": None,
            "ABCDFF.SRINV.V0": None,
            "ABCDFF2.CEUSED.V0": None,
            "ABCDFF2.SRUSED.V0": None,
            "FFMUXA1.D6": None,
            "FFMUXA2.BYP": None,
        },
        "INT_X1Y0": {
            "PIP.INT_NODE_IMUX_42_INT_OUT1.LOGIC_OUTS_W14": None,
            "PIP.BYPASS_W5.INT_NODE_IMUX_42_INT_OUT1": None,
            "PIP.INT_NODE_IMUX_48_INT_OUT0.BYPASS_W5": None,
            "PIP.IMUX_W10.INT_NODE_IMUX_48_INT_OUT0": None,
            "PIP.INT_NODE_IMUX_40_INT_OUT1.LOGIC_OUTS_W12": None,
            "PIP.BYPASS_W6.INT_NODE_IMUX_40_INT_OUT1": None,
            "PIP.INODE_W_9_FT1.BYPASS_W6": None,
            "PIP.BOUNCE_W_0_FT1.INODE_W_9_FT1": None,
        },
    }
