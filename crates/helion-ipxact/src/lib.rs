//! IEEE 1685-style pack/reimport (minimal XML). Helion-MM/ST, not AXI.

use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IpCore {
    pub vendor: String,
    pub library: String,
    pub name: String,
    pub version: String,
    pub bus: String,
}

pub fn pack_uart() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_uart".into(),
        version: "1.0".into(),
        bus: "Helion-MM".into(),
    }
}

pub fn pack_gpio() -> IpCore {
    IpCore {
        vendor: "community".into(),
        library: "helion".into(),
        name: "h_gpio".into(),
        version: "1.0".into(),
        bus: "Helion-MM".into(),
    }
}

pub fn to_xml(ip: &IpCore) -> String {
    format!(
        r#"<?xml version="1.0"?>
<ipxact:component xmlns:ipxact="http://www.accellera.org/XMLSchema/IPXACT/1685-2014">
  <ipxact:vendor>{}</ipxact:vendor>
  <ipxact:library>{}</ipxact:library>
  <ipxact:name>{}</ipxact:name>
  <ipxact:version>{}</ipxact:version>
  <ipxact:busInterfaces>
    <ipxact:busInterface>
      <ipxact:name>s_mm</ipxact:name>
      <ipxact:description>{}</ipxact:description>
    </ipxact:busInterface>
  </ipxact:busInterfaces>
</ipxact:component>
"#,
        ip.vendor, ip.library, ip.name, ip.version, ip.bus
    )
}

pub fn from_xml(xml: &str) -> Result<IpCore, String> {
    let grab = |tag: &str| {
        let open = format!("<ipxact:{tag}>");
        let close = format!("</ipxact:{tag}>");
        xml.split_once(&open)
            .and_then(|(_, r)| r.split_once(&close))
            .map(|(v, _)| v.trim().to_string())
            .ok_or_else(|| format!("missing {tag}"))
    };
    Ok(IpCore {
        vendor: grab("vendor")?,
        library: grab("library")?,
        name: grab("name")?,
        version: grab("version")?,
        bus: grab("description").unwrap_or_else(|_| "Helion-MM".into()),
    })
}

pub fn write_core(dir: &Path, ip: &IpCore) -> Result<std::path::PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let p = dir.join(format!("{}.xml", ip.name));
    std::fs::write(&p, to_xml(ip)).map_err(|e| e.to_string())?;
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uart_pack_reimport_helion_mm() {
        let ip = pack_uart();
        assert_eq!(ip.bus, "Helion-MM");
        assert_ne!(ip.bus, "AXI");
        let xml = to_xml(&ip);
        let back = from_xml(&xml).unwrap();
        assert_eq!(ip, back);
        let gpio = pack_gpio();
        assert_eq!(gpio.name, "h_gpio");
        assert_eq!(gpio.bus, "Helion-MM");
    }
}
