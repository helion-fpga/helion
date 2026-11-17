use helion_ir::{intern, Attrs, Cell, CellKind, Design};

#[test]
fn frozen_pins_answer_like_the_scan() {
    let mut d = Design::new("t");
    d.push_cell(Cell { name: intern("a"), kind: CellKind::Lut6 { init: 1 }, attrs: Attrs::default() });
    d.connect("n", "a", "I0");
    let before = (d.net_on(intern("a"), intern("I0")), d.net_on(intern("a"), intern("I1")));
    let mut f = d.clone_data();
    f.freeze_pins();
    assert_eq!(before, (Some(intern("n")), None));
    assert_eq!((f.net_on(intern("a"), intern("I0")), f.net_on(intern("a"), intern("I1"))), before);
}
