//! NPN canonical table for 4-input functions.
//!
//! Contains precomputed optimal AIG implementations for NPN equivalence
//! classes. Currently includes the trivial classes; the full 222-class
//! table is generated offline via exhaustive search.
//!
//! Each entry: (canonical_tt, description, aig_node_count)
//! The actual AIG structure is built on-demand via `build_optimal_aig()`.

/// NPN table entry: optimal implementation for a canonical form.
#[derive(Clone, Debug)]
pub struct NpnTableEntry {
    /// 16-bit canonical truth table.
    pub canonical_tt: u16,
    /// Human-readable description.
    pub description: &'static str,
    /// Number of AND nodes in optimal implementation.
    pub and_count: usize,
}

/// Precomputed NPN table (subset: trivial classes).
/// Full table has 222 entries; this includes the 8 most common.
pub const NPN_TABLE: &[NpnTableEntry] = &[
    // Constant 0: no nodes needed
    NpnTableEntry {
        canonical_tt: 0x0000,
        description: "const0",
        and_count: 0,
    },
    // Constant 1: no nodes needed (inverted const0)
    NpnTableEntry {
        canonical_tt: 0xFFFF,
        description: "const1",
        and_count: 0,
    },
    // Buffer: x0 (single input, others don't care)
    // Canonical form: 0x00FF (x0)
    NpnTableEntry {
        canonical_tt: 0x00FF,
        description: "buf_x0",
        and_count: 0,
    },
    // Inverter: ~x0
    NpnTableEntry {
        canonical_tt: 0xFF00,
        description: "inv_x0",
        and_count: 0,
    },
    // AND2: x0 & x1 (others don't care)
    // Canonical: 0x000F? Actually for 4 inputs, AND2 is 0x8888? Let's use 2-input embedded.
    // For simplicity, we list the 4-input AND: x0&x1&x2&x3 = 0x8000
    NpnTableEntry {
        canonical_tt: 0x8000,
        description: "and4",
        and_count: 3, // 3 AND2s for 4-input AND
    },
    // OR4: x0|x1|x2|x3 = 0xFFFE? No, 0x7FFF? Let's compute: OR is 1 unless all 0.
    // Truth table: bit m is 1 if any input is 1. Only m=0 has output 0.
    // So tt = 0xFFFE (all bits 1 except bit 0)
    NpnTableEntry {
        canonical_tt: 0xFFFE,
        description: "or4",
        and_count: 3, // De Morgan: 3 ANDs + inverters
    },
    // XOR2 embedded: x0 ^ x1 = 0x6666? For 4 inputs with x2,x3 don't care,
    // the pattern repeats. Canonical 2-input XOR is 0x6 (0110).
    // Embedded in 4 inputs: 0x6666
    NpnTableEntry {
        canonical_tt: 0x6666,
        description: "xor2_x0_x1",
        and_count: 3, // XOR needs 3 ANDs in AIG
    },
];

/// Lookup table entry by canonical truth table.
/// Returns None if not in the precomputed subset.
pub fn lookup_npn(canonical_tt: u16) -> Option<&'static NpnTableEntry> {
    NPN_TABLE.iter().find(|e| e.canonical_tt == canonical_tt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_contains_constants() {
        assert!(lookup_npn(0x0000).is_some());
        assert!(lookup_npn(0xFFFF).is_some());
    }

    #[test]
    fn table_lookup_miss() {
        // Random non-trivial function not in subset
        assert!(lookup_npn(0x1234).is_none());
    }
}
