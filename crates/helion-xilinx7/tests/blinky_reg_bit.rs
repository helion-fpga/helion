//! End-to-end for the *registered* ZedBoard blinky (the layout that matches
//! the hardware-proven reference): `count <= count + 1; LD0 <= count(24)`.
//!
//! The route gap this test guards is the one that kept LD0 static on the
//! board while every structural check passed: `count_24` reached the LD0
//! register's raw FF-D endpoint but never the paired slice's lane LUT input
//! (`CLBLM_L_D6` <- `INT_R_X71Y44.IMUX42`), so the `FFMUX.XOR` sum the
//! register actually samples was constant.
//!
//! The assertion is on the produced artifact (FASM + assembled `.bit`), not
//! on an internal wire: `INT_R_X71Y44.IMUX42` must have an arm in the image
//! an actual programmer would receive.

use std::path::Path;

use helion_xilinx7::bitstream::{self, BitstreamOptions};
use helion_xilinx7::device::Device;
use helion_xilinx7::fasm::parse_fasm;
use helion_xilinx7::frames::{
    BlockType, FrameAddr, FrameStore, SegBitsDb, TileBitLayout,
};
use helion_xilinx7::xdevice::{build_node_map, XDevice};
use helion_xilinx7::{emit_fasm, pack, place, route};
use serde_json::Value;

const SRC: &str = r#"
module blinky (
    input  wire clk,
    output reg  LD0
);
    reg [24:0] count = 25'd0;

    always @(posedge clk) begin
        count <= count + 1'b1;
        LD0   <= count[24];
    end
endmodule
"#;

const XDC: &str = "\
set_property PACKAGE_PIN Y9 [get_ports {clk}]\n\
set_property IOSTANDARD LVCMOS33 [get_ports {clk}]\n\
set_property PACKAGE_PIN T22 [get_ports {LD0}]\n\
set_property IOSTANDARD LVCMOS33 [get_ports {LD0}]\n";

const IDCODE_XC7Z020: u32 = 0x0372_7093;

struct TileEntry {
    tile_type: String,
    blocks: std::collections::HashMap<BlockType, (FrameAddr, u32)>,
}

struct TilegridLayout {
    tiles: std::collections::HashMap<String, TileEntry>,
}

impl TilegridLayout {
    fn load(tilegrid: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(tilegrid).ok()?;
        let json: Value = serde_json::from_str(&text).ok()?;
        let obj = json.as_object()?;
        let mut tiles = std::collections::HashMap::new();
        for (name, info) in obj {
            let tile_type = info.get("type")?.as_str()?.to_string();
            let mut blocks = std::collections::HashMap::new();
            if let Some(bits) = info.get("bits").and_then(|b| b.as_object()) {
                for (bus, b) in bits {
                    let block = match bus.as_str() {
                        "CLB_IO_CLK" => BlockType::ClbIoClk,
                        "BLOCK_RAM" => BlockType::BlockRam,
                        _ => continue,
                    };
                    let baseaddr = b.get("baseaddr")?.as_str()?;
                    let baseaddr =
                        u32::from_str_radix(baseaddr.strip_prefix("0x").unwrap_or(baseaddr), 16)
                            .ok()?;
                    let offset = b.get("offset")?.as_u64()? as u32;
                    let frames = b.get("frames")?.as_u64()? as u32;
                    blocks.insert(block, (FrameAddr { baseaddr, offset }, frames));
                }
            }
            tiles.insert(name.clone(), TileEntry { tile_type, blocks });
        }
        Some(TilegridLayout { tiles })
    }
}

impl TileBitLayout for TilegridLayout {
    fn frame_addr(&self, tile: &str, block: BlockType) -> Option<FrameAddr> {
        Some(self.tiles.get(tile)?.blocks.get(&block)?.0)
    }

    fn tile_type(&self, tile: &str) -> Option<&str> {
        Some(self.tiles.get(tile)?.tile_type.as_str())
    }
}

fn load_segbits(zynq7: &Path, types: &[&str]) -> SegBitsDb {
    let mut db = SegBitsDb::new();
    for t in types {
        let lower = t.to_lowercase();
        let seg = zynq7.join(format!("segbits_{lower}.db"));
        if seg.is_file() {
            db.add_segbits(t, BlockType::ClbIoClk, &std::fs::read_to_string(&seg).unwrap())
                .unwrap();
        }
        let ppips = zynq7.join(format!("ppips_{lower}.db"));
        if ppips.is_file() {
            db.add_ppips(t, &std::fs::read_to_string(&ppips).unwrap()).unwrap();
        }
    }
    db
}

#[test]
fn registered_blinky_image_routes_the_ld0_operand() {
    let db_root = std::env::var("HELION_XILINX_DB")
        .map(std::path::PathBuf::from)
        .expect("HELION_XILINX_DB must point at a prjxray-db checkout");
    let zynq7 = db_root.join("zynq7");
    let device = Device::load_part_from_db(&db_root, "xc7z020").expect("load xc7z020");
    let nm = build_node_map(&mut device.clone());

    let mut design = helion_sv::synth_sv(SRC, "blinky_reg.sv").expect("synth");
    helion_xilinx7::build::PinConstraints::parse(XDC).apply(&mut design);

    let packed = pack(&design).expect("pack");
    let mut xdev = XDevice::new(&device, nm);
    for b in &packed.bufgs {
        xdev.clock_nets.insert(b.out_net.clone());
    }
    let placed = place(&packed, &xdev).expect("place");
    let routed = route(&placed, &xdev).expect("route");
    let bs = emit_fasm(&routed, &xdev).expect("emit");

    // The route expectation, on the thing a programmer receives as FASM:
    // the LD0 register's source bit must arm an input mux at
    // INT_R_X71Y44.IMUX42 (the paired slice's D6 operand pin).
    let imux42_arm = bs
        .fasm
        .iter()
        .find(|l| l.starts_with("INT_R_X71Y44.IMUX42."))
        .cloned()
        .expect("INT_R_X71Y44.IMUX42 has no arm: the LD0 operand route gap is back");
    println!("LD0 operand arm: {imux42_arm}");

    // FASM -> frames -> .bit.
    let fasm_text = bs.fasm.join("\n");
    let lines = parse_fasm(&fasm_text).expect("parse our FASM");
    let layout = TilegridLayout::load(&zynq7.join("xc7z020/tilegrid.json")).expect("tilegrid");
    let mut types: Vec<&str> = lines.iter().filter_map(|l| layout.tile_type(&l.tile)).collect();
    types.sort();
    types.dedup();
    let segbits = load_segbits(&zynq7, &types);
    // `*_SING` tiles ship no segbits and must not borrow them (see AGENTS);
    // LD0/T22 is not on a *_SING tile, so an empty keep-set is the norm here.
    let lines: Vec<_> = lines
        .into_iter()
        .filter(|l| layout.tile_type(&l.tile).map(|t| !t.contains("_SING")).unwrap_or(true))
        .collect();
    let mut store = FrameStore::for_part(device.part()).expect("frame store");
    assert_eq!(store.frame_count(), 9_996, "xc7z020 has 9996 frames");
    let applied = store.apply_fasm(&lines, &segbits, &layout).expect("apply FASM");
    assert!(applied > 0, "no FASM features applied");
    // The undescribed clock-network word Vivado also writes; no segbit names
    // it, so it cannot arrive through `apply_fasm` (see `frames.rs`).
    let _word50 = helion_xilinx7::frames::apply_clock_word_50(&mut store);
    let payload = store.fdri_payload();
    let opts = BitstreamOptions {
        idcode: IDCODE_XC7Z020,
        part_name: "xc7z020clg484-1".to_string(),
        design_source: "blinky_reg;Generator=helion-xilinx7".to_string(),
        ..BitstreamOptions::xc7frames2bit("xc7z020clg484-1", "blinky_reg.frm", "2026/10/24", "00:00:00")
    };
    let bit = bitstream::write_bit(&payload, &opts).expect("write .bit");
    let idcode_bytes = IDCODE_XC7Z020.to_be_bytes();
    assert!(bit.windows(4).any(|w| w == idcode_bytes), "IDCODE missing");
    let out = std::path::PathBuf::from("/tmp/x7/blinky_reg_fixed.bit");
    std::fs::create_dir_all(out.parent().unwrap()).ok();
    std::fs::write(&out, &bit).unwrap();
    println!("wrote {} ({} bytes)", out.display(), bit.len());
}
