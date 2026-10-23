//! Slice targets on the left-hand side of an assignment.
//!
//! `sig[hi:lo] = rhs` assigns to bits lo..=hi. Discarding the bounds made
//! every slice write land at slice-relative bit 0, so disjoint slice writes
//! collapsed onto the same bits.

use std::collections::HashMap;

fn net_endpoints(src: &str) -> HashMap<String, usize> {
    let d = helion_sv::synth_sv(src, "t.sv").expect("synth");
    d.nets
        .iter()
        .map(|n| (n.name.clone(), n.endpoints.len()))
        .collect()
}

#[test]
fn disjoint_slices_drive_distinct_bits() {
    let eps = net_endpoints(
        r#"module s1(input logic [7:0] a, output logic [7:0] y);
             assign y[1:0]=a[1:0]; assign y[3:2]=a[3:2];
             assign y[5:4]=a[5:4]; assign y[7:6]=a[7:6];
           endmodule"#,
    );
    for i in 0..8 {
        let n = format!("y_{i}");
        assert_eq!(
            eps.get(&n).copied().unwrap_or(0),
            1,
            "`{n}` must have exactly one driver/load, got {:?}",
            eps.get(&n)
        );
    }
}

#[test]
fn slices_do_not_alias_each_other() {
    // Four writes collapsing to one offset would give the low bits a large
    // endpoint count and leave the high bits absent.
    let eps = net_endpoints(
        r#"module s2(input logic [3:0] a, output logic [7:0] y);
             assign y[3:0]=a; assign y[7:4]=a;
           endmodule"#,
    );
    assert_eq!(eps.get("y_0").copied().unwrap_or(0), 1, "y_0: {eps:?}");
    assert_eq!(eps.get("y_7").copied().unwrap_or(0), 1, "y_7: {eps:?}");
}

#[test]
fn attributes_in_a_port_list_do_not_delete_ports() {
    let d = helion_sv::synth_sv(
        r#"module s3((* keep = "true" *) input logic a, output logic y, output logic z);
             assign y = ~a; assign z = a;
           endmodule"#,
        "t.sv",
    )
    .expect("synth");
    let mut ports: Vec<_> = d.ports.iter().map(|p| p.name.clone()).collect();
    ports.sort();
    assert_eq!(ports, vec!["a", "y", "z"]);
}
