use helion_ipxact::{
    BayerPattern, ColorStandard, CscDirection, CscScalerConfig, HdmiStandard, HdmiTmdsConfig,
    MipiCsi2Config, VdmaConfig, VideoResolutionPreset, VideoTimingMode, VtcConfig, catalog,
    generate_axi_vdma_sv, generate_csc_scaler_sv, generate_hdmi_tmds_rx_sv,
    generate_hdmi_tmds_tx_sv, generate_mipi_csi2_rx_demosaic_sv, generate_vtc_sv,
    pack_hdmi_tmds_rx, pack_hdmi_tmds_tx, pack_mipi_csi2_rx, pack_video_csc_scaler,
    pack_video_vdma, pack_video_vtc,
};

#[test]
fn video_ip_catalog_and_synthesizable_sv_generators_cover_all_five_cores() {
    let cat = catalog();
    for core in [
        pack_hdmi_tmds_tx(),
        pack_hdmi_tmds_rx(),
        pack_video_vdma(),
        pack_video_vtc(),
        pack_mipi_csi2_rx(),
        pack_video_csc_scaler(),
    ] {
        assert!(
            cat.contains(&core),
            "catalog must include video IP {}",
            core.name
        );
    }

    // 1. HDMI 1.4 & 2.0 TMDS TX/RX SV generation
    let hdmi_cfg = HdmiTmdsConfig {
        module_name: "h_hdmi_tmds".into(),
        standard: HdmiStandard::Hdmi20,
        bpc: 8,
        enable_infoframe: true,
    };
    let tx_sv = generate_hdmi_tmds_tx_sv(&hdmi_cfg);
    let rx_sv = generate_hdmi_tmds_rx_sv(&hdmi_cfg);
    assert!(tx_sv.contains("module h_hdmi_tmds_tx"));
    assert!(tx_sv.contains("terc4_encode"));
    assert!(tx_sv.contains("HDMI20_SCRAMBLE = 1'b1"));
    assert!(rx_sv.contains("module h_hdmi_tmds_rx"));
    assert!(rx_sv.contains("module h_hdmi_tmds_lane_dec"));

    // 2. PG020 Triple-Buffered Video DMA SV generation
    let vdma_sv = generate_axi_vdma_sv(&VdmaConfig::default());
    assert!(vdma_sv.contains("module h_video_vdma"));
    assert!(vdma_sv.contains("s2mm_fsync"));
    assert!(vdma_sv.contains("mm2s_fsync"));
    assert!(vdma_sv.contains("NUM_BUFFERS = 3"));

    // 3. PG016 Video Timing Controller presets (480p, 720p, 1080p, 4K) & SV generation
    let m480 = VideoTimingMode::from_preset(VideoResolutionPreset::P480);
    assert_eq!((m480.h_total(), m480.v_total()), (800, 525));
    let m720 = VideoTimingMode::from_preset(VideoResolutionPreset::P720);
    assert_eq!((m720.h_total(), m720.v_total()), (1650, 750));
    let m1080 = VideoTimingMode::from_preset(VideoResolutionPreset::P1080);
    assert_eq!((m1080.h_total(), m1080.v_total()), (2200, 1125));
    let m4k = VideoTimingMode::from_preset(VideoResolutionPreset::Uhd4K);
    assert_eq!((m4k.h_total(), m4k.v_total()), (4400, 2250));

    let vtc_sv = generate_vtc_sv(&VtcConfig {
        module_name: "h_video_vtc".into(),
        default_preset: VideoResolutionPreset::Uhd4K,
        enable_generator: true,
        enable_detector: true,
    });
    assert!(vtc_sv.contains("module h_video_vtc"));
    assert!(vtc_sv.contains("H_ACTIVE     = 3840"));
    assert!(vtc_sv.contains("V_ACTIVE     = 2160"));
    assert!(vtc_sv.contains("det_locked"));

    // 4. MIPI CSI-2 D-PHY RX + Bayer Demosaic SV generation
    let csi_sv = generate_mipi_csi2_rx_demosaic_sv(&MipiCsi2Config {
        module_name: "h_mipi_csi2_rx".into(),
        num_lanes: 4,
        pixel_bits: 10,
        line_width: 1920,
        bayer_pattern: BayerPattern::Grbg,
    });
    assert!(csi_sv.contains("module h_mipi_csi2_rx"));
    assert!(csi_sv.contains("NUM_LANES  = 4"));
    assert!(csi_sv.contains("calc_csi2_ecc"));
    assert!(csi_sv.contains("line_buf0"));

    // 5. RGB/YCbCr Color Space Converter & Bilinear Scaler SV generation
    let csc_sv = generate_csc_scaler_sv(&CscScalerConfig {
        module_name: "h_video_csc_scaler".into(),
        standard: ColorStandard::Bt2020,
        direction: CscDirection::YCbCrToRgb,
        in_width: 1920,
        in_height: 1080,
        out_width: 3840,
        out_height: 2160,
        enable_scaler: true,
    });
    assert!(csc_sv.contains("module h_video_csc_scaler"));
    assert!(csc_sv.contains("DIRECTION_YCBCR_TO_RGB = 1'b1"));
    assert!(csc_sv.contains("OUT_WIDTH              = 3840"));
}
