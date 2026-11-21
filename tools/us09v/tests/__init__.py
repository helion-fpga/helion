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
