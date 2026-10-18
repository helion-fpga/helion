//! Vivado UG994 Block Design graph, interface connections, and connection automation.

use crate::BlockDesign;
use helion_ipxact::{
    BusInterface, BusInterfaceType, InterfaceMode, IpComponent, IpCore, PortDirection,
    ResetPolarity, component_axi_interconnect, component_axi_protocol_converter, find_ip_component,
    upgrade_core_to_component,
};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::sync::{LazyLock, RwLock};

/// Instance of an IP component inside a BlockDesign (`create_bd_cell`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BdCell {
    pub instance_name: String,
    pub vlnv: String,
    pub component: IpComponent,
    pub properties: BTreeMap<String, String>,
    pub clock_domain: Option<String>,
    pub reset_domain: Option<String>,
}

impl BdCell {
    pub fn get_interface(&self, name: &str) -> Option<&BusInterface> {
        self.component.get_interface(name)
    }
}

/// External scalar/vector port on the BlockDesign boundary (`create_bd_port`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BdPort {
    pub name: String,
    pub direction: PortDirection,
    pub width: u32,
    pub port_type: Option<BusInterfaceType>,
    pub clock_domain: Option<String>,
    pub reset_polarity: Option<ResetPolarity>,
}

/// External bus interface port on the BlockDesign boundary (`create_bd_intf_port`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BdIntfPort {
    pub name: String,
    pub bus_type: BusInterfaceType,
    pub mode: InterfaceMode,
    pub clock_domain: Option<String>,
}

/// Scalar/vector net connecting pins and external ports (`connect_bd_net`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BdNet {
    pub name: String,
    /// Endpoints formatted as `"cell_inst/pin_name"` or `"port_name"`.
    pub endpoints: Vec<String>,
    pub clock_domain: Option<String>,
}

/// Interface-level connection between two interface endpoints (`connect_bd_intf_net`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BdIntfNet {
    pub name: String,
    /// Endpoint formatted as `"cell_inst/intf_name"` or `"intf_port_name"`.
    pub endpoints: Vec<String>,
    pub bus_type: BusInterfaceType,
}

/// Memory map address segment assigned to a slave interface in a master's address space.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BdAddressSegment {
    pub name: String,
    pub master_cell: String,
    pub master_intf: String,
    pub slave_cell: String,
    pub slave_intf: String,
    pub base_address: u64,
    pub range: u64,
}

impl BdAddressSegment {
    pub fn high_address(&self) -> u64 {
        self.base_address.saturating_add(self.range.saturating_sub(1))
    }
}

/// Connection automation rule matching Vivado `apply_bd_automation -rule ...`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BdAutomationRule {
    /// `xilinx.com:bd_rule:axi4`: connect slave AXI interface to master (inserting interconnect/converter as needed) and wire clock/reset.
    Axi4 {
        target_cell: String,
        target_intf: String,
        master: Option<String>,
        clk_source: Option<String>,
        rst_source: Option<String>,
    },
    /// `xilinx.com:bd_rule:clkrst`: automatically wire clock and reset inputs across cells.
    ClkRst {
        clk_source: String,
        rst_source: String,
        clock_domain: Option<String>,
    },
    /// `xilinx.com:bd_rule:board`: expose an external interface port (GPIO, UART, SPI, I2C, BRAM_PORT) on the top-level boundary.
    Board {
        target_cell: String,
        target_intf: String,
        external_port_name: Option<String>,
    },
    /// `xilinx.com:bd_rule:bram_cntlr`: connect `axi_bram_ctrl/BRAM_PORTA` to a `blk_mem_gen` instance.
    BramCntlr {
        target_cell: String,
        target_intf: String,
    },
}

/// Internal rich state of a `BlockDesign`.
#[derive(Clone, Debug, Default)]
pub struct BdGraph {
    pub name: String,
    pub is_explicit_bd: bool,
    pub synced_cores: Vec<IpCore>,
    pub cells: Vec<BdCell>,
    pub ports: Vec<BdPort>,
    pub intf_ports: Vec<BdIntfPort>,
    pub nets: Vec<BdNet>,
    pub intf_nets: Vec<BdIntfNet>,
    pub address_segments: Vec<BdAddressSegment>,
}

impl BdGraph {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.into(),
            is_explicit_bd: true,
            ..Default::default()
        }
    }

    pub fn find_cell(&self, inst: &str) -> Option<&BdCell> {
        self.cells
            .iter()
            .find(|c| c.instance_name.eq_ignore_ascii_case(inst))
    }

    pub fn find_cell_mut(&mut self, inst: &str) -> Option<&mut BdCell> {
        self.cells
            .iter_mut()
            .find(|c| c.instance_name.eq_ignore_ascii_case(inst))
    }

    /// Rebuild an interconnect cell's `IpComponent` after its master/slave count changes.
    pub fn refresh_interconnect_cell(&mut self, inst: &str) {
        let Some(cell) = self.find_cell_mut(inst) else {
            return;
        };
        if cell.component.core.name == "axi_interconnect"
            || cell.component.core.name == "axi_crossbar"
        {
            let m = cell
                .component
                .get_param_i64(&cell.properties, "C_NUM_MASTERS")
                .max(1) as u32;
            let s = cell
                .component
                .get_param_i64(&cell.properties, "C_NUM_SLAVES")
                .max(1) as u32;
            let mut new_comp = component_axi_interconnect(m, s);
            new_comp.core.name = cell.component.core.name.clone();
            cell.component = new_comp;
        }
    }
}

thread_local! {
    static THREAD_BD_REGISTRY: RefCell<HashMap<String, BdGraph>> = RefCell::new(HashMap::new());
}

static GLOBAL_BD_REG: LazyLock<RwLock<HashMap<String, BdGraph>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

fn global_bd_registry() -> &'static RwLock<HashMap<String, BdGraph>> {
    &GLOBAL_BD_REG
}

pub fn register_bd_graph(bd: &mut BlockDesign, mut graph: BdGraph) {
    graph.name = bd.name.clone();
    bd.cores = graph.cells.iter().map(|c| c.component.core.clone()).collect();
    graph.synced_cores = bd.cores.clone();
    THREAD_BD_REGISTRY.with(|reg| {
        reg.borrow_mut().insert(bd.name.clone(), graph.clone());
    });
    if let Ok(mut greg) = global_bd_registry().write() {
        greg.insert(bd.name.clone(), graph);
    }
}

/// Retrieve the `BdGraph` for `bd`. If `bd` was constructed via struct literal `BlockDesign { name, cores }`,
/// returns `None` so legacy `Helion-MM`/`Helion-ST` callers preserve their exact behavior.
pub fn get_explicit_bd_graph(bd: &BlockDesign) -> Option<BdGraph> {
    let from_thread = THREAD_BD_REGISTRY.with(|reg| reg.borrow().get(&bd.name).cloned());
    let candidate = from_thread.or_else(|| {
        global_bd_registry()
            .read()
            .ok()
            .and_then(|g| g.get(&bd.name).cloned())
    });
    if let Some(graph) = candidate {
        if graph.is_explicit_bd && graph.synced_cores == bd.cores {
            return Some(graph);
        }
    }
    None
}

/// Retrieve or synthesize a `BdGraph` for `bd`.
pub fn get_or_build_bd_graph(bd: &BlockDesign) -> BdGraph {
    if let Some(graph) = get_explicit_bd_graph(bd) {
        return graph;
    }
    let mut graph = BdGraph::new(&bd.name);
    graph.is_explicit_bd = false;
    for core in &bd.cores {
        let comp = upgrade_core_to_component(core);
        graph.cells.push(BdCell {
            instance_name: format!("u_{}", core.name),
            vlnv: core.vlnv(),
            component: comp,
            properties: BTreeMap::new(),
            clock_domain: Some("clk".into()),
            reset_domain: Some("resetn".into()),
        });
    }
    graph.synced_cores = bd.cores.clone();
    graph
}

pub fn with_bd_graph_mut<F, R>(bd: &mut BlockDesign, f: F) -> R
where
    F: FnOnce(&mut BdGraph) -> R,
{
    let mut graph = get_or_build_bd_graph(bd);
    graph.is_explicit_bd = true;
    let res = f(&mut graph);
    register_bd_graph(bd, graph);
    res
}

/// Create a new empty Vivado UG994 `BlockDesign` (`create_bd_design <name>`).
pub fn create_bd_design(name: &str) -> BlockDesign {
    let mut bd = BlockDesign {
        name: name.into(),
        cores: Vec::new(),
    };
    let mut graph = BdGraph::new(name);
    // Default top-level clock and reset ports are available if not overridden
    graph.ports.push(BdPort {
        name: "aclk".into(),
        direction: PortDirection::In,
        width: 1,
        port_type: Some(BusInterfaceType::Clock),
        clock_domain: Some("aclk".into()),
        reset_polarity: None,
    });
    graph.ports.push(BdPort {
        name: "aresetn".into(),
        direction: PortDirection::In,
        width: 1,
        port_type: Some(BusInterfaceType::Reset),
        clock_domain: Some("aclk".into()),
        reset_polarity: Some(ResetPolarity::ActiveLow),
    });
    register_bd_graph(&mut bd, graph);
    bd
}

/// Instantiate an IP cell in `bd` (`create_bd_cell -type ip -vlnv <vlnv_or_name> <instance_name>`).
pub fn create_bd_cell(
    bd: &mut BlockDesign,
    vlnv_or_name: &str,
    instance_name: &str,
) -> Result<String, String> {
    let inst = instance_name.trim();
    if inst.is_empty() {
        return Err("create_bd_cell: empty instance name".into());
    }
    let comp = find_ip_component(vlnv_or_name)
        .ok_or_else(|| format!("create_bd_cell: unknown IP '{vlnv_or_name}'"))?;
    with_bd_graph_mut(bd, |graph| {
        if graph.find_cell(inst).is_some() {
            return Err(format!("create_bd_cell: cell '{inst}' already exists"));
        }
        let vlnv = comp.vlnv();
        graph.cells.push(BdCell {
            instance_name: inst.into(),
            vlnv: vlnv.clone(),
            component: comp,
            properties: BTreeMap::new(),
            clock_domain: None,
            reset_domain: None,
        });
        Ok(format!("/{inst}"))
    })
}

/// Create an external scalar/vector port on the block design boundary (`create_bd_port`).
pub fn create_bd_port(
    bd: &mut BlockDesign,
    name: &str,
    direction: PortDirection,
    width: u32,
    port_type: Option<BusInterfaceType>,
) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("create_bd_port: empty port name".into());
    }
    with_bd_graph_mut(bd, |graph| {
        if let Some(existing) = graph.ports.iter_mut().find(|p| p.name == name) {
            existing.direction = direction;
            existing.width = width.max(1);
            existing.port_type = port_type;
            if port_type == Some(BusInterfaceType::Clock) {
                existing.clock_domain = Some(name.into());
            }
            return Ok(format!("/{name}"));
        }
        let clock_domain = if port_type == Some(BusInterfaceType::Clock) {
            Some(name.into())
        } else {
            None
        };
        let reset_polarity = if port_type == Some(BusInterfaceType::Reset) {
            if name.to_ascii_lowercase().ends_with('n') {
                Some(ResetPolarity::ActiveLow)
            } else {
                Some(ResetPolarity::ActiveHigh)
            }
        } else {
            None
        };
        graph.ports.push(BdPort {
            name: name.into(),
            direction,
            width: width.max(1),
            port_type,
            clock_domain,
            reset_polarity,
        });
        Ok(format!("/{name}"))
    })
}

/// Create an external interface port on the block design boundary (`create_bd_intf_port`).
pub fn create_bd_intf_port(
    bd: &mut BlockDesign,
    name: &str,
    bus_type: BusInterfaceType,
    mode: InterfaceMode,
) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("create_bd_intf_port: empty port name".into());
    }
    with_bd_graph_mut(bd, |graph| {
        if !graph.intf_ports.iter().any(|p| p.name == name) {
            graph.intf_ports.push(BdIntfPort {
                name: name.into(),
                bus_type,
                mode,
                clock_domain: None,
            });
        }
        Ok(format!("/{name}"))
    })
}

/// Set `CONFIG.*` property on a cell (`set_property CONFIG.<key> <val> [get_bd_cells <inst>]`).
pub fn set_bd_cell_property(
    bd: &mut BlockDesign,
    instance_name: &str,
    key: &str,
    value: &str,
) -> Result<(), String> {
    let inst = instance_name.trim().trim_start_matches('/');
    let norm_key = key.trim().strip_prefix("CONFIG.").unwrap_or(key.trim());
    with_bd_graph_mut(bd, |graph| {
        let cell = graph
            .find_cell_mut(inst)
            .ok_or_else(|| format!("set_property: unknown cell '{inst}'"))?;
        cell.properties.insert(norm_key.into(), value.trim().into());
        graph.refresh_interconnect_cell(inst);
        Ok(())
    })
}

/// Normalize an endpoint string (strips leading `/`).
pub fn norm_ep(ep: &str) -> String {
    ep.trim().trim_start_matches('/').to_string()
}

/// Connect two or more scalar/vector pins or ports (`connect_bd_net`).
/// Also propagates clock and reset domains automatically!
pub fn connect_bd_net(bd: &mut BlockDesign, net_name: Option<&str>, endpoints: &[&str]) -> Result<String, String> {
    let clean_eps: Vec<String> = endpoints
        .iter()
        .map(|e| norm_ep(e))
        .filter(|e| !e.is_empty())
        .collect();
    if clean_eps.len() < 2 {
        return Err("connect_bd_net: requires at least 2 endpoints".into());
    }
    with_bd_graph_mut(bd, |graph| {
        connect_bd_net_in_graph(graph, net_name, &clean_eps)
    })
}

pub fn connect_bd_net_in_graph(
    graph: &mut BdGraph,
    net_name: Option<&str>,
    clean_eps: &[String],
) -> Result<String, String> {
    let existing_idx = if let Some(explicit) = net_name.map(str::trim).filter(|s| !s.is_empty()) {
        graph
            .nets
            .iter()
            .position(|n| n.name.eq_ignore_ascii_case(explicit))
    } else {
        graph.nets.iter().position(|n| {
            n.endpoints
                .iter()
                .any(|x| x.eq_ignore_ascii_case(&clean_eps[0]))
        })
    };

    // Detach any cell-pin endpoints from other nets before attaching to the target net
    for (idx, net) in graph.nets.iter_mut().enumerate() {
        if Some(idx) != existing_idx {
            net.endpoints.retain(|existing_ep| {
                !(existing_ep.contains('/')
                    && clean_eps
                        .iter()
                        .any(|ep| ep.eq_ignore_ascii_case(existing_ep)))
            });
        }
    }

    let chosen_name = if let Some(idx) = existing_idx {
        for ep in clean_eps {
            if !graph.nets[idx].endpoints.iter().any(|x| x.eq_ignore_ascii_case(ep)) {
                graph.nets[idx].endpoints.push(ep.clone());
            }
        }
        graph.nets[idx].name.clone()
    } else {
        let name = net_name
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| format!("net_{}_{}", clean_eps[0].replace('/', "_"), graph.nets.len()));
        graph.nets.push(BdNet {
            name: name.clone(),
            endpoints: clean_eps.to_vec(),
            clock_domain: None,
        });
        name
    };

    // Propagate clock and reset domains across all endpoints on this net
    propagate_domains_for_net(graph, &chosen_name);
    Ok(chosen_name)
}

fn propagate_domains_for_net(graph: &mut BdGraph, net_name: &str) {
    let Some(net_idx) = graph.nets.iter().position(|n| n.name == net_name) else {
        return;
    };
    let eps = graph.nets[net_idx].endpoints.clone();
    // Discover any clock source or reset source among endpoints
    let mut clk_domain: Option<String> = graph.nets[net_idx].clock_domain.clone();
    let mut rst_domain: Option<String> = None;

    for ep in &eps {
        if !ep.contains('/') {
            // Top-level port
            if let Some(p) = graph.ports.iter().find(|p| p.name.eq_ignore_ascii_case(ep)) {
                if p.port_type == Some(BusInterfaceType::Clock)
                    || p.name.to_ascii_lowercase().contains("clk")
                {
                    clk_domain = Some(p.clock_domain.clone().unwrap_or_else(|| p.name.clone()));
                } else if p.port_type == Some(BusInterfaceType::Reset)
                    || p.name.to_ascii_lowercase().contains("rst")
                    || p.name.to_ascii_lowercase().contains("reset")
                {
                    rst_domain = Some(p.name.clone());
                }
            } else {
                // Implicit top-level port created on the fly if referenced
                let lower = ep.to_ascii_lowercase();
                if lower.contains("clk") {
                    clk_domain = Some(ep.clone());
                } else if lower.contains("rst") || lower.contains("reset") {
                    rst_domain = Some(ep.clone());
                }
            }
        } else if let Some((cell_inst, pin_name)) = ep.split_once('/') {
            if let Some(cell) = graph.find_cell(cell_inst) {
                // Check if pin is a clock master or reset master (e.g., proc_sys_reset)
                for intf in &cell.component.interfaces {
                    let matches_pin = intf.name.eq_ignore_ascii_case(pin_name)
                        || intf
                            .port_maps
                            .iter()
                            .any(|pm| pm.physical_name.eq_ignore_ascii_case(pin_name));
                    if matches_pin && intf.mode == InterfaceMode::Master {
                        if intf.bus_type == BusInterfaceType::Clock {
                            clk_domain = Some(
                                cell.clock_domain
                                    .clone()
                                    .unwrap_or_else(|| format!("{cell_inst}_{pin_name}")),
                            );
                        } else if intf.bus_type == BusInterfaceType::Reset {
                            rst_domain = Some(format!("{cell_inst}/{pin_name}"));
                        }
                    }
                }
            }
        }
    }

    if let Some(dom) = &clk_domain {
        graph.nets[net_idx].clock_domain = Some(dom.clone());
        for ep in &eps {
            if let Some((cell_inst, pin_name)) = ep.split_once('/') {
                if let Some(cell) = graph.find_cell_mut(cell_inst) {
                    let lower = pin_name.to_ascii_lowercase();
                    if lower.contains("clk") || lower == "clka" {
                        cell.clock_domain = Some(dom.clone());
                    }
                }
            }
        }
    }
    if let Some(rdom) = &rst_domain {
        for ep in &eps {
            if let Some((cell_inst, pin_name)) = ep.split_once('/') {
                if let Some(cell) = graph.find_cell_mut(cell_inst) {
                    let lower = pin_name.to_ascii_lowercase();
                    if lower.contains("rst") || lower.contains("reset") {
                        cell.reset_domain = Some(rdom.clone());
                    }
                }
            }
        }
    }
}

/// Resolve an interface endpoint (`"cell/intf"` or `"intf_port"`) to `(BusInterfaceType, InterfaceMode)`.
pub fn resolve_intf_endpoint(
    graph: &BdGraph,
    ep: &str,
) -> Result<(BusInterfaceType, InterfaceMode), String> {
    let ep = norm_ep(ep);
    if let Some((cell_inst, intf_name)) = ep.split_once('/') {
        let cell = graph
            .find_cell(cell_inst)
            .ok_or_else(|| format!("unknown cell '{cell_inst}' in endpoint '{ep}'"))?;
        let intf = cell.get_interface(intf_name).ok_or_else(|| {
            format!(
                "cell '{}' ({}) has no bus interface '{}'",
                cell_inst, cell.component.core.name, intf_name
            )
        })?;
        Ok((intf.bus_type, intf.mode))
    } else if let Some(port) = graph
        .intf_ports
        .iter()
        .find(|p| p.name.eq_ignore_ascii_case(&ep))
    {
        // An external Master port acts as a Slave sink to an internal Master, and vice-versa
        let effective_mode = match port.mode {
            InterfaceMode::Master => InterfaceMode::Slave,
            InterfaceMode::Slave => InterfaceMode::Master,
            other => other,
        };
        Ok((port.bus_type, effective_mode))
    } else {
        Err(format!("unknown interface endpoint '{ep}'"))
    }
}

/// Connect two interface endpoints (`connect_bd_intf_net`).
/// Validates interface compatibility and automatically inserts `axi_protocol_converter` when
/// bridging `AXI4` (burst) to `AXI4-Lite` (single-beat).
pub fn connect_bd_intf_net(
    bd: &mut BlockDesign,
    intf_net_name: Option<&str>,
    ep1: &str,
    ep2: &str,
) -> Result<String, String> {
    with_bd_graph_mut(bd, |graph| {
        connect_bd_intf_net_in_graph(graph, intf_net_name, ep1, ep2)
    })
}

pub fn connect_bd_intf_net_in_graph(
    graph: &mut BdGraph,
    intf_net_name: Option<&str>,
    ep1: &str,
    ep2: &str,
) -> Result<String, String> {
    let e1 = norm_ep(ep1);
    let e2 = norm_ep(ep2);
    let (bt1, m1) = resolve_intf_endpoint(graph, &e1)?;
    let (bt2, m2) = resolve_intf_endpoint(graph, &e2)?;

    if !m1.is_compatible_with(m2) {
        return Err(format!(
            "connect_bd_intf_net: mode mismatch between '{e1}' ({m1}) and '{e2}' ({m2})"
        ));
    }

    // If connecting AXI4 Master to AXI4-Lite Slave directly, automatically insert axi_protocol_converter!
    if (bt1 == BusInterfaceType::Axi4 && bt2 == BusInterfaceType::Axi4Lite && m1 == InterfaceMode::Master)
        || (bt2 == BusInterfaceType::Axi4 && bt1 == BusInterfaceType::Axi4Lite && m2 == InterfaceMode::Master)
    {
        let (master_ep, slave_ep) = if m1 == InterfaceMode::Master {
            (e1, e2)
        } else {
            (e2, e1)
        };
        let conv_inst = format!("auto_pc_{}", graph.cells.len());
        let comp = component_axi_protocol_converter();
        // Inherit clock/reset domain from master cell
        let (m_clk, m_rst) = master_ep
            .split_once('/')
            .and_then(|(c, _)| graph.find_cell(c))
            .map(|c| (c.clock_domain.clone(), c.reset_domain.clone()))
            .unwrap_or((None, None));
        graph.cells.push(BdCell {
            instance_name: conv_inst.clone(),
            vlnv: comp.vlnv(),
            component: comp,
            properties: BTreeMap::new(),
            clock_domain: m_clk,
            reset_domain: m_rst,
        });
        let n1 = format!("intf_{}_to_{conv_inst}", master_ep.replace('/', "_"));
        graph.intf_nets.push(BdIntfNet {
            name: n1,
            endpoints: vec![master_ep, format!("{conv_inst}/S_AXI")],
            bus_type: BusInterfaceType::Axi4,
        });
        let n2 = intf_net_name
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| format!("intf_{conv_inst}_to_{}", slave_ep.replace('/', "_")));
        graph.intf_nets.push(BdIntfNet {
            name: n2.clone(),
            endpoints: vec![format!("{conv_inst}/M_AXI"), slave_ep],
            bus_type: BusInterfaceType::Axi4Lite,
        });
        return Ok(n2);
    }

    if bt1 != bt2 {
        return Err(format!(
            "connect_bd_intf_net: incompatible interface types '{e1}' ({bt1}) and '{e2}' ({bt2})"
        ));
    }

    let name = intf_net_name
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            format!(
                "intf_{}_{}_{}",
                e1.replace('/', "_"),
                e2.replace('/', "_"),
                graph.intf_nets.len()
            )
        });
    graph.intf_nets.push(BdIntfNet {
        name: name.clone(),
        endpoints: vec![e1, e2],
        bus_type: bt1,
    });
    Ok(name)
}

/// Apply Vivado UG994 connection automation (`apply_bd_automation`):
/// - Automatically inserts or expands `axi_interconnect` (and `axi_protocol_converter` if any master is `AXI4`)
///   when connecting $M$ masters to $S$ slaves!
/// - Automatically wires clock and reset domains across the master, interconnect, and slave cells!
/// - Supports `ClkRst`, `Board` (external GPIO/UART/SPI/I2C/BRAM_PORT port creation), and `BramCntlr` rules!
pub fn apply_bd_automation(bd: &mut BlockDesign, rule: BdAutomationRule) -> Result<String, String> {
    with_bd_graph_mut(bd, |graph| match rule {
        BdAutomationRule::Axi4 {
            target_cell,
            target_intf,
            master,
            clk_source,
            rst_source,
        } => apply_axi4_automation_in_graph(
            graph,
            &target_cell,
            &target_intf,
            master.as_deref(),
            clk_source.as_deref(),
            rst_source.as_deref(),
        ),
        BdAutomationRule::ClkRst {
            clk_source,
            rst_source,
            clock_domain,
        } => apply_clkrst_automation_in_graph(
            graph,
            &clk_source,
            &rst_source,
            clock_domain.as_deref(),
        ),
        BdAutomationRule::Board {
            target_cell,
            target_intf,
            external_port_name,
        } => apply_board_automation_in_graph(
            graph,
            &target_cell,
            &target_intf,
            external_port_name.as_deref(),
        ),
        BdAutomationRule::BramCntlr {
            target_cell,
            target_intf,
        } => apply_bram_automation_in_graph(graph, &target_cell, &target_intf),
    })
}

fn apply_axi4_automation_in_graph(
    graph: &mut BdGraph,
    target_cell: &str,
    target_intf: &str,
    master_spec: Option<&str>,
    clk_source: Option<&str>,
    rst_source: Option<&str>,
) -> Result<String, String> {
    let slave_cell = norm_ep(target_cell);
    let slave_intf = norm_ep(target_intf);
    let slave_ep = format!("{slave_cell}/{slave_intf}");
    // Validate slave exists
    let _ = resolve_intf_endpoint(graph, &slave_ep)?;

    // Discover all master endpoints to connect (either specified master or all AXI master cells in design)
    let mut master_eps: Vec<String> = Vec::new();
    if let Some(m) = master_spec {
        let m_norm = norm_ep(m);
        if m_norm.contains('/') {
            master_eps.push(m_norm);
        } else if let Some(cell) = graph.find_cell(&m_norm) {
            if let Some(intf) = cell
                .component
                .interfaces
                .iter()
                .find(|i| i.mode == InterfaceMode::Master && i.bus_type.is_axi_mm())
            {
                master_eps.push(format!("{}/{}", cell.instance_name, intf.name));
            }
        }
    }
    if master_eps.is_empty() {
        for cell in &graph.cells {
            if cell.component.core.name == "axi_interconnect"
                || cell.component.core.name == "axi_crossbar"
                || cell.component.core.name == "axi_protocol_converter"
            {
                continue;
            }
            for intf in &cell.component.interfaces {
                if intf.mode == InterfaceMode::Master && intf.bus_type.is_axi_mm() {
                    master_eps.push(format!("{}/{}", cell.instance_name, intf.name));
                }
            }
        }
    }
    if master_eps.is_empty() {
        return Err("apply_bd_automation: no AXI master found in BlockDesign".into());
    }

    // Ensure an `axi_interconnect` exists in the design (or create `axi_interconnect_0`)
    let ic_inst = if let Some(existing) = graph.cells.iter().find(|c| {
        c.component.core.name == "axi_interconnect" || c.component.core.name == "axi_crossbar"
    }) {
        existing.instance_name.clone()
    } else {
        let inst = "axi_interconnect_0".to_string();
        let comp = component_axi_interconnect(master_eps.len() as u32, 1);
        graph.cells.push(BdCell {
            instance_name: inst.clone(),
            vlnv: comp.vlnv(),
            component: comp,
            properties: BTreeMap::new(),
            clock_domain: None,
            reset_domain: None,
        });
        inst
    };

    // Connect every discovered master to S00_AXI, S01_AXI, ... on `ic_inst` if not already connected
    for (m_idx, m_ep) in master_eps.iter().enumerate() {
        let already_connected = graph
            .intf_nets
            .iter()
            .any(|n| n.endpoints.iter().any(|e| e.eq_ignore_ascii_case(m_ep)));
        if !already_connected {
            let ic_s_port = format!("{ic_inst}/S{m_idx:02}_AXI");
            // Ensure interconnect has at least m_idx + 1 master ports
            if let Some(ic_cell) = graph.find_cell_mut(&ic_inst) {
                let cur_m = ic_cell
                    .component
                    .get_param_i64(&ic_cell.properties, "C_NUM_MASTERS")
                    .max(1) as usize;
                if m_idx + 1 > cur_m {
                    ic_cell
                        .properties
                        .insert("C_NUM_MASTERS".into(), (m_idx + 1).to_string());
                }
            }
            graph.refresh_interconnect_cell(&ic_inst);
            connect_bd_intf_net_in_graph(graph, None, m_ep, &ic_s_port)?;
        }
    }

    // Check if slave_ep is already connected; if not, allocate next free Mxx_AXI slot on `ic_inst`
    let slave_already = graph
        .intf_nets
        .iter()
        .any(|n| n.endpoints.iter().any(|e| e.eq_ignore_ascii_case(&slave_ep)));
    if !slave_already {
        let used_m_slots = graph
            .intf_nets
            .iter()
            .filter(|n| {
                n.endpoints
                    .iter()
                    .any(|e| e.starts_with(&format!("{ic_inst}/M")))
            })
            .count();
        let slot_idx = used_m_slots;
        if let Some(ic_cell) = graph.find_cell_mut(&ic_inst) {
            let cur_s = ic_cell
                .component
                .get_param_i64(&ic_cell.properties, "C_NUM_SLAVES")
                .max(1) as usize;
            if slot_idx + 1 > cur_s {
                ic_cell
                    .properties
                    .insert("C_NUM_SLAVES".into(), (slot_idx + 1).to_string());
            }
            if slot_idx == 0 {
                ic_cell.properties.insert("C_NUM_SLAVES".into(), "1".into());
            }
        }
        graph.refresh_interconnect_cell(&ic_inst);
        let ic_m_port = format!("{ic_inst}/M{slot_idx:02}_AXI");
        connect_bd_intf_net_in_graph(graph, None, &ic_m_port, &slave_ep)?;
    }

    // Wire clock and reset for master, interconnect, and target slave
    let clk_src = clk_source.unwrap_or("aclk");
    let rst_src_owned = if let Some(s) = rst_source {
        s.to_string()
    } else if let Some(psr) = graph
        .cells
        .iter()
        .find(|c| c.component.core.name == "proc_sys_reset")
    {
        format!("{}/peripheral_aresetn", psr.instance_name)
    } else {
        "aresetn".to_string()
    };
    apply_clkrst_automation_in_graph(graph, clk_src, &rst_src_owned, Some(clk_src))?;

    Ok(format!("apply_bd_automation: connected {slave_ep} via {ic_inst}"))
}

fn apply_clkrst_automation_in_graph(
    graph: &mut BdGraph,
    clk_source: &str,
    rst_source: &str,
    clock_domain: Option<&str>,
) -> Result<String, String> {
    let clk_src = norm_ep(clk_source);
    let rst_src = norm_ep(rst_source);
    let dom = clock_domain.unwrap_or(&clk_src).to_string();

    let mut clk_eps = vec![clk_src.clone()];
    let mut rst_eps = vec![rst_src.clone()];

    for cell in &mut graph.cells {
        if cell.clock_domain.is_none() {
            cell.clock_domain = Some(dom.clone());
        }
        if cell.reset_domain.is_none() {
            cell.reset_domain = Some(rst_src.clone());
        }
        for intf in &cell.component.interfaces {
            if intf.mode == InterfaceMode::Slave {
                if intf.bus_type == BusInterfaceType::Clock {
                    for pm in &intf.port_maps {
                        clk_eps.push(format!("{}/{}", cell.instance_name, pm.physical_name));
                    }
                } else if intf.bus_type == BusInterfaceType::Reset {
                    for pm in &intf.port_maps {
                        let ep = format!("{}/{}", cell.instance_name, pm.physical_name);
                        if ep != rst_src {
                            rst_eps.push(ep);
                        }
                    }
                }
            }
        }
    }

    if clk_eps.len() >= 2 {
        connect_bd_net_in_graph(graph, Some("clk_net"), &clk_eps)?;
    }
    if rst_eps.len() >= 2 {
        connect_bd_net_in_graph(graph, Some("rst_net"), &rst_eps)?;
    }
    Ok(format!(
        "apply_bd_automation: clkrst wired clk={clk_src} rst={rst_src}"
    ))
}

fn apply_board_automation_in_graph(
    graph: &mut BdGraph,
    target_cell: &str,
    target_intf: &str,
    external_port_name: Option<&str>,
) -> Result<String, String> {
    let cell_inst = norm_ep(target_cell);
    let intf_name = norm_ep(target_intf);
    let ep = format!("{cell_inst}/{intf_name}");
    let (bus_type, mode) = resolve_intf_endpoint(graph, &ep)?;
    let ext_name = external_port_name
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("{}_{}", cell_inst, intf_name.to_ascii_lowercase()));
    if !graph.intf_ports.iter().any(|p| p.name == ext_name) {
        graph.intf_ports.push(BdIntfPort {
            name: ext_name.clone(),
            bus_type,
            mode,
            clock_domain: None,
        });
    }
    connect_bd_intf_net_in_graph(graph, None, &ep, &ext_name)?;
    Ok(format!("apply_bd_automation: exported {ep} to /{ext_name}"))
}

fn apply_bram_automation_in_graph(
    graph: &mut BdGraph,
    target_cell: &str,
    target_intf: &str,
) -> Result<String, String> {
    let cell_inst = norm_ep(target_cell);
    let intf_name = norm_ep(target_intf);
    let ep = format!("{cell_inst}/{intf_name}");
    let bmg_inst = format!("{cell_inst}_bram");
    if graph.find_cell(&bmg_inst).is_none() {
        let comp = find_ip_component("blk_mem_gen").unwrap();
        let (clk_dom, rst_dom) = graph
            .find_cell(&cell_inst)
            .map(|c| (c.clock_domain.clone(), c.reset_domain.clone()))
            .unwrap_or((None, None));
        graph.cells.push(BdCell {
            instance_name: bmg_inst.clone(),
            vlnv: comp.vlnv(),
            component: comp,
            properties: BTreeMap::new(),
            clock_domain: clk_dom,
            reset_domain: rst_dom,
        });
    }
    connect_bd_intf_net_in_graph(graph, None, &ep, &format!("{bmg_inst}/BRAM_PORTA"))?;
    Ok(format!(
        "apply_bd_automation: connected {ep} to {bmg_inst}/BRAM_PORTA"
    ))
}
