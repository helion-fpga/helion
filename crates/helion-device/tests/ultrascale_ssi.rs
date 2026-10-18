use helion_device::ultrascale::{
    Carry8Chain, Carry8Mode, Dsp48e2Config, Dsp48e2WideXorMode, LagunaCrossingMode, PblockSlrSpec,
    SlrCascadeKind, SlrCascadeSpan, UltraScaleDevice, UltraScalePart, plan_uram288_cascade,
    sll_crossing_delay_ps, validate_pblock_slr_assignment,
};

#[test]
fn ultrascale_parts_and_4slr_vu13p_ssi_topology_and_cascades() {
    for (name, expected_part, expected_slrs) in [
        ("xcku5p-ffvb676-2-e", UltraScalePart::Xcku5p, 1),
        ("xczu9eg-ffvb1156-2-e", UltraScalePart::Xczu9eg, 1),
        ("xcvu9p-flga2104-2L-e", UltraScalePart::Xcvu9p, 3),
        ("xcvu13p-fhga2104-2L-e", UltraScalePart::Xcvu13p, 4),
    ] {
        let p = UltraScalePart::parse(name).expect("valid UltraScale+ part");
        assert_eq!(p, expected_part);
        let dev = UltraScaleDevice::for_part(p);
        assert_eq!(dev.slr_count, expected_slrs);
        assert_eq!(dev.lut6_count, dev.clb_slices * 8);
        assert_eq!(dev.ff_count, dev.clb_slices * 16);
    }

    // xcvu13p 4-SLR SSI validation: 3 interposer cuts (SLR0<->SLR1, SLR1<->SLR2, SLR2<->SLR3)
    let vu13p = UltraScaleDevice::for_part(UltraScalePart::Xcvu13p);
    let topo = vu13p.slr_topology();
    assert_eq!(topo.num_slrs(), 4);
    assert_eq!(topo.num_interposer_cuts(), 3);
    assert_eq!(topo.slr_hops_between_y(10, 900), Some(3));
    assert!(
        sll_crossing_delay_ps(0, 3, LagunaCrossingMode::TxRxRegistered, 40.0)
            > sll_crossing_delay_ps(0, 1, LagunaCrossingMode::TxRxRegistered, 40.0)
    );

    // Multi-column URAM288 cascade (100 blocks > 64 per SLR column => requires 2 SLR columns)
    let cas_plan = plan_uram288_cascade(&vu13p, 100 * 4096, 72, 4).unwrap();
    assert_eq!(cas_plan.depth_cascade_len, 100);
    assert_eq!(cas_plan.slr_columns_needed, 2);

    // Verify SLR-1 (pblock target_slr mismatch) and SLR-2 (resource overflow)
    let bad_pb = PblockSlrSpec {
        name: "pb_overflow".into(),
        target_slr: Some(0),
        y_min: 0,
        y_max: 300, // crosses into SLR1 while pinned to target_slr=Some(0) -> SLR-1
        req_clb_slices: 999_999, // exceeds SLR0..SLR1 CLB capacity -> SLR-2
        req_dsp48e2: 100,
        req_ramb36e2: 100,
        req_uram288: 100,
    };
    let dsp_cross = SlrCascadeSpan {
        name: "dsp_cross_slr".into(),
        kind: SlrCascadeKind::Dsp48e2,
        y_start: 470, // SLR1
        y_end: 490,   // SLR2 -> SLR-3
    };
    let rep = validate_pblock_slr_assignment(&topo, &[bad_pb], &[dsp_cross], &[1000, 1000, 1000]);
    assert!(rep.has_code("SLR-1"));
    assert!(rep.has_code("SLR-2"));
    assert!(rep.has_code("SLR-3"));

    // Verify CARRY8 and DSP48E2 wide XOR modes
    let cy = Carry8Chain::new(Carry8Mode::SingleCy8);
    let (sum, co) = cy.evaluate(false, false, 0x00, 0b1010_1010);
    assert_eq!(sum, 0b1010_1010);
    assert_eq!(co, 0x00);

    let dsp_cfg = Dsp48e2Config {
        use_preadder: true,
        preadder_subtract: true,
        use_mult: true,
        pipeline_stages: 3,
        wide_xor: Dsp48e2WideXorMode::Xor12,
        pattern: 0,
        mask: 0xFFFF_FFFF_FFFF,
    };
    let dsp_res = helion_device::dsp48e2_evaluate(&dsp_cfg, 3, 7, 10, 20, true);
    // (20 - 3) * 7 + 10 + 1 = 119 + 11 = 130
    assert_eq!(dsp_res.p, 130);
    assert!(dsp_res.patdet);
}
