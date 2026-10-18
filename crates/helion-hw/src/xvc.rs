//! Xilinx Virtual Cable (XVC 1.0) wire protocol server & client (XAPP1251 / UG908).
//!
//! Supports the complete XVC 1.0 binary command set:
//! - `getinfo:` -> `"xvcServer_v1.0:<max_vector_len>\n"`
//! - `settck:<period_ns:u32_le>` -> `<actual_period_ns:u32_le>`
//! - `shift:<num_bits:u32_le><tms_bytes><tdi_bytes>` -> `<tdo_bytes>`
//!
//! Works both in-process against [`Tap`] and over TCP sockets (`TcpListener` / `TcpStream`)
//! for remote network board programming, IDCODE/STAT readback, and JTAG-SPI flash tunneling.

use crate::{Bitstream, IR_CFG_W, IR_IDCODE, IR_STAT, Stat, Tap};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};

pub const XVC_VERSION: &str = "xvcServer_v1.0";
pub const XVC_DEFAULT_MAX_VECTOR_LEN: u32 = 32768;
pub const XVC_DEFAULT_TCK_NS: u32 = 100;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct XvcServerInfo {
    pub version: String,
    pub max_vector_len: u32,
}

/// XVC 1.0 protocol server backed by a Helion IEEE 1149.1 [`Tap`].
#[derive(Clone, Debug)]
pub struct XvcServer {
    pub tap: Tap,
    pub tck_period_ns: u32,
    pub max_vector_len: u32,
    pub shift_ops: u64,
    pub total_bits_shifted: u64,
}

impl XvcServer {
    pub fn new(tap: Tap) -> Self {
        Self {
            tap,
            tck_period_ns: XVC_DEFAULT_TCK_NS,
            max_vector_len: XVC_DEFAULT_MAX_VECTOR_LEN,
            shift_ops: 0,
            total_bits_shifted: 0,
        }
    }

    pub fn with_max_vector_len(mut self, max_len: u32) -> Self {
        self.max_vector_len = max_len.max(64);
        self
    }

    pub fn tap(&self) -> &Tap {
        &self.tap
    }

    pub fn tap_mut(&mut self) -> &mut Tap {
        &mut self.tap
    }

    /// Execute one XVC command from `buf`.
    /// Returns `(response_bytes, bytes_consumed_from_buf)`.
    pub fn handle_packet(&mut self, buf: &[u8]) -> Result<(Vec<u8>, usize), String> {
        if buf.starts_with(b"getinfo:") {
            let reply = format!("{}:{}\n", XVC_VERSION, self.max_vector_len);
            return Ok((reply.into_bytes(), 8));
        }
        if buf.starts_with(b"settck:") {
            if buf.len() < 11 {
                return Err("xvc settck: truncated 4-byte period".into());
            }
            let req_period = u32::from_le_bytes([buf[7], buf[8], buf[9], buf[10]]);
            self.tck_period_ns = req_period.max(10);
            return Ok((self.tck_period_ns.to_le_bytes().to_vec(), 11));
        }
        if buf.starts_with(b"shift:") {
            if buf.len() < 10 {
                return Err("xvc shift: truncated 4-byte num_bits".into());
            }
            let num_bits = u32::from_le_bytes([buf[6], buf[7], buf[8], buf[9]]);
            let num_bytes = ((num_bits as usize) + 7) / 8;
            let total_cmd_len = 10 + 2 * num_bytes;
            if (num_bytes * 2) as u32 > self.max_vector_len {
                return Err(format!(
                    "xvc shift: vector bytes {} exceeds max_vector_len {}",
                    num_bytes * 2,
                    self.max_vector_len
                ));
            }
            if buf.len() < total_cmd_len {
                return Err(format!(
                    "xvc shift: expected {total_cmd_len} bytes for {num_bits} bits, got {}",
                    buf.len()
                ));
            }
            let tms_bytes = &buf[10..10 + num_bytes];
            let tdi_bytes = &buf[10 + num_bytes..10 + 2 * num_bytes];
            let tdo_bytes = self.exec_shift(num_bits, tms_bytes, tdi_bytes);
            return Ok((tdo_bytes, total_cmd_len));
        }
        Err("xvc: unrecognized command prefix (expected getinfo:, settck:, or shift:)".into())
    }

    /// Step the underlying [`Tap`] for `num_bits` TCK cycles, LSB-first within each byte.
    pub fn exec_shift(&mut self, num_bits: u32, tms_bytes: &[u8], tdi_bytes: &[u8]) -> Vec<u8> {
        let num_bytes = ((num_bits as usize) + 7) / 8;
        let mut tdo_bytes = vec![0u8; num_bytes];
        for bit_i in 0..(num_bits as usize) {
            let byte_ix = bit_i / 8;
            let bit_off = bit_i % 8;
            let tms = ((tms_bytes.get(byte_ix).copied().unwrap_or(0) >> bit_off) & 1) != 0;
            let tdi = ((tdi_bytes.get(byte_ix).copied().unwrap_or(0) >> bit_off) & 1) != 0;
            let tdo = self.tap.tick(tms, tdi);
            if tdo {
                tdo_bytes[byte_ix] |= 1u8 << bit_off;
            }
        }
        self.shift_ops = self.shift_ops.saturating_add(1);
        self.total_bits_shifted = self.total_bits_shifted.saturating_add(u64::from(num_bits));
        tdo_bytes
    }

    /// Serve a connected stream (e.g. `TcpStream`) until EOF.
    pub fn serve_stream<R: Read, W: Write>(
        &mut self,
        reader: &mut R,
        writer: &mut W,
    ) -> Result<usize, String> {
        let mut cmds_handled = 0usize;
        loop {
            let mut prefix = [0u8; 1];
            match reader.read(&mut prefix) {
                Ok(0) => break,
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(format!("xvc stream read: {e}")),
            }
            // Read until ':'
            let mut cmd_head = vec![prefix[0]];
            while *cmd_head.last().unwrap() != b':' {
                let mut b = [0u8; 1];
                reader
                    .read_exact(&mut b)
                    .map_err(|e| format!("xvc header read: {e}"))?;
                cmd_head.push(b[0]);
                if cmd_head.len() > 16 {
                    return Err("xvc: header colon not found within 16 bytes".into());
                }
            }
            match cmd_head.as_slice() {
                b"getinfo:" => {
                    let (resp, _) = self.handle_packet(&cmd_head)?;
                    writer
                        .write_all(&resp)
                        .map_err(|e| format!("xvc write getinfo: {e}"))?;
                    writer.flush().map_err(|e| format!("xvc flush: {e}"))?;
                }
                b"settck:" => {
                    let mut p = [0u8; 4];
                    reader
                        .read_exact(&mut p)
                        .map_err(|e| format!("xvc settck read: {e}"))?;
                    cmd_head.extend_from_slice(&p);
                    let (resp, _) = self.handle_packet(&cmd_head)?;
                    writer
                        .write_all(&resp)
                        .map_err(|e| format!("xvc write settck: {e}"))?;
                    writer.flush().map_err(|e| format!("xvc flush: {e}"))?;
                }
                b"shift:" => {
                    let mut nbits_buf = [0u8; 4];
                    reader
                        .read_exact(&mut nbits_buf)
                        .map_err(|e| format!("xvc shift nbits read: {e}"))?;
                    let num_bits = u32::from_le_bytes(nbits_buf);
                    let num_bytes = ((num_bits as usize) + 7) / 8;
                    if (num_bytes * 2) as u32 > self.max_vector_len {
                        return Err("xvc shift exceeds max_vector_len".into());
                    }
                    let mut vec_buf = vec![0u8; 2 * num_bytes];
                    reader
                        .read_exact(&mut vec_buf)
                        .map_err(|e| format!("xvc shift vector read: {e}"))?;
                    cmd_head.extend_from_slice(&nbits_buf);
                    cmd_head.extend_from_slice(&vec_buf);
                    let (resp, _) = self.handle_packet(&cmd_head)?;
                    writer
                        .write_all(&resp)
                        .map_err(|e| format!("xvc write shift: {e}"))?;
                    writer.flush().map_err(|e| format!("xvc flush: {e}"))?;
                }
                other => {
                    return Err(format!(
                        "xvc unknown command {:?}",
                        String::from_utf8_lossy(other)
                    ));
                }
            }
            cmds_handled += 1;
        }
        Ok(cmds_handled)
    }
}

/// Handle for a background TCP XVC 1.0 server thread.
pub struct XvcTcpServerHandle {
    pub addr: SocketAddr,
    pub server: Arc<Mutex<XvcServer>>,
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl XvcTcpServerHandle {
    pub fn shutdown(mut self) -> XvcServer {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.addr);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
        match self.server.lock() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }
}

/// Spawn a TCP XVC 1.0 server on `bind_addr` (e.g. `"127.0.0.1:0"` for ephemeral port).
pub fn spawn_xvc_tcp_server(
    tap: Tap,
    bind_addr: &str,
) -> Result<(SocketAddr, XvcTcpServerHandle), String> {
    let listener =
        TcpListener::bind(bind_addr).map_err(|e| format!("xvc bind {bind_addr}: {e}"))?;
    let local_addr = listener
        .local_addr()
        .map_err(|e| format!("xvc local_addr: {e}"))?;
    let server = Arc::new(Mutex::new(XvcServer::new(tap)));
    let server_bg = Arc::clone(&server);
    let stop = Arc::new(AtomicBool::new(false));
    let stop_bg = Arc::clone(&stop);

    let join = thread::spawn(move || {
        for incoming in listener.incoming() {
            if stop_bg.load(Ordering::SeqCst) {
                break;
            }
            if let Ok(mut stream) = incoming {
                if stop_bg.load(Ordering::SeqCst) {
                    break;
                }
                let _ = stream.set_nodelay(true);
                let mut reader = match stream.try_clone() {
                    Ok(r) => r,
                    Err(_) => continue,
                };
                if let Ok(mut srv) = server_bg.lock() {
                    let _ = srv.serve_stream(&mut reader, &mut stream);
                }
            }
        }
    });

    Ok((
        local_addr,
        XvcTcpServerHandle {
            addr: local_addr,
            server,
            stop,
            join: Some(join),
        },
    ))
}

/// Transport endpoint for [`XvcClient`]: either an in-process [`XvcServer`] or a TCP socket.
pub enum XvcTransport {
    InProcess(XvcServer),
    Tcp(TcpStream),
}

/// XVC 1.0 client driving `getinfo:`, `settck:`, and `shift:` over TCP or in-process transport.
pub struct XvcClient {
    pub transport: XvcTransport,
    pub max_vector_len: u32,
    pub tck_period_ns: u32,
}

impl XvcClient {
    pub fn from_server(server: XvcServer) -> Self {
        let max_vector_len = server.max_vector_len;
        let tck_period_ns = server.tck_period_ns;
        Self {
            transport: XvcTransport::InProcess(server),
            max_vector_len,
            tck_period_ns,
        }
    }

    pub fn connect(addr: SocketAddr) -> Result<Self, String> {
        let stream = TcpStream::connect(addr).map_err(|e| format!("xvc connect {addr}: {e}"))?;
        let _ = stream.set_nodelay(true);
        let mut client = Self {
            transport: XvcTransport::Tcp(stream),
            max_vector_len: XVC_DEFAULT_MAX_VECTOR_LEN,
            tck_period_ns: XVC_DEFAULT_TCK_NS,
        };
        let info = client.get_info()?;
        client.max_vector_len = info.max_vector_len;
        Ok(client)
    }

    pub fn into_server(self) -> Option<XvcServer> {
        match self.transport {
            XvcTransport::InProcess(s) => Some(s),
            XvcTransport::Tcp(_) => None,
        }
    }

    pub fn server_mut(&mut self) -> Option<&mut XvcServer> {
        match &mut self.transport {
            XvcTransport::InProcess(s) => Some(s),
            XvcTransport::Tcp(_) => None,
        }
    }

    /// Issue `getinfo:` and parse `"xvcServer_v1.0:<max_vector_len>\n"`.
    pub fn get_info(&mut self) -> Result<XvcServerInfo, String> {
        let raw = match &mut self.transport {
            XvcTransport::InProcess(srv) => srv.handle_packet(b"getinfo:")?.0,
            XvcTransport::Tcp(stream) => {
                stream
                    .write_all(b"getinfo:")
                    .map_err(|e| format!("xvc getinfo write: {e}"))?;
                stream.flush().map_err(|e| format!("xvc flush: {e}"))?;
                let mut line = Vec::new();
                let mut b = [0u8; 1];
                while b[0] != b'\n' {
                    stream
                        .read_exact(&mut b)
                        .map_err(|e| format!("xvc getinfo read: {e}"))?;
                    line.push(b[0]);
                }
                line
            }
        };
        let s = String::from_utf8(raw).map_err(|e| format!("xvc getinfo utf8: {e}"))?;
        let trimmed = s.trim();
        let (ver, len_str) = trimmed
            .split_once(':')
            .ok_or_else(|| format!("malformed xvc getinfo response: {trimmed}"))?;
        let max_vector_len = len_str
            .parse::<u32>()
            .map_err(|_| format!("invalid xvc max_vector_len: {len_str}"))?;
        self.max_vector_len = max_vector_len;
        Ok(XvcServerInfo {
            version: ver.to_string(),
            max_vector_len,
        })
    }

    /// Issue `settck:<period_ns>` and return the server's applied TCK period in nanoseconds.
    pub fn set_tck(&mut self, period_ns: u32) -> Result<u32, String> {
        let mut msg = Vec::with_capacity(11);
        msg.extend_from_slice(b"settck:");
        msg.extend_from_slice(&period_ns.to_le_bytes());
        let resp = match &mut self.transport {
            XvcTransport::InProcess(srv) => srv.handle_packet(&msg)?.0,
            XvcTransport::Tcp(stream) => {
                stream
                    .write_all(&msg)
                    .map_err(|e| format!("xvc settck write: {e}"))?;
                stream.flush().map_err(|e| format!("xvc flush: {e}"))?;
                let mut out = [0u8; 4];
                stream
                    .read_exact(&mut out)
                    .map_err(|e| format!("xvc settck read: {e}"))?;
                out.to_vec()
            }
        };
        let actual = u32::from_le_bytes([resp[0], resp[1], resp[2], resp[3]]);
        self.tck_period_ns = actual;
        Ok(actual)
    }

    /// Issue `shift:<num_bits><tms_bytes><tdi_bytes>` and return `<tdo_bytes>`.
    pub fn shift(&mut self, num_bits: u32, tms: &[u8], tdi: &[u8]) -> Result<Vec<u8>, String> {
        let num_bytes = ((num_bits as usize) + 7) / 8;
        if tms.len() < num_bytes || tdi.len() < num_bytes {
            return Err("xvc shift: tms/tdi slice shorter than required num_bytes".into());
        }
        let mut msg = Vec::with_capacity(10 + 2 * num_bytes);
        msg.extend_from_slice(b"shift:");
        msg.extend_from_slice(&num_bits.to_le_bytes());
        msg.extend_from_slice(&tms[..num_bytes]);
        msg.extend_from_slice(&tdi[..num_bytes]);
        match &mut self.transport {
            XvcTransport::InProcess(srv) => Ok(srv.handle_packet(&msg)?.0),
            XvcTransport::Tcp(stream) => {
                stream
                    .write_all(&msg)
                    .map_err(|e| format!("xvc shift write: {e}"))?;
                stream.flush().map_err(|e| format!("xvc flush: {e}"))?;
                let mut tdo = vec![0u8; num_bytes];
                stream
                    .read_exact(&mut tdo)
                    .map_err(|e| format!("xvc shift read: {e}"))?;
                Ok(tdo)
            }
        }
    }

    /// Reset TAP to `RunTestIdle` via 5× TMS=1 (`TestLogicReset`) followed by 1× TMS=0 (`RunTestIdle`).
    pub fn reset_to_idle(&mut self) -> Result<(), String> {
        // 6 bits: bits 0..4 have TMS=1 (0b01_1111 = 0x1F), bit 5 has TMS=0 -> lands in RunTestIdle.
        self.shift(6, &[0x1F], &[0x00])?;
        Ok(())
    }

    /// Shift a 6-bit IEEE 1149.1 instruction register (`IR_*`) from `RunTestIdle` and return to `RunTestIdle`.
    pub fn shift_ir(&mut self, ir_opcode: u8) -> Result<u8, String> {
        self.reset_to_idle()?;
        // Navigate RunTestIdle -> SelectDr(1) -> SelectIr(1) -> CaptureIr(0) -> ShiftIr(0): 4 bits, TMS = 0b0011 = 0x03
        self.shift(4, &[0x03], &[0x00])?;
        // Shift 6 IR bits: first 5 bits TMS=0, 6th bit TMS=1 (Exit1Ir): TMS = 0b10_0000 = 0x20
        let tdo = self.shift(6, &[0x20], &[ir_opcode & 0x3F])?;
        // Exit1Ir -> UpdateIr(1) -> RunTestIdle(0): 2 bits, TMS = 0b01 = 0x01
        self.shift(2, &[0x01], &[0x00])?;
        Ok(tdo[0] & 0x3F)
    }

    /// Shift a 32-bit DR scan from `RunTestIdle` and return captured 32-bit TDO word.
    pub fn shift_dr_u32(&mut self, tdi_word: u32) -> Result<u32, String> {
        // RunTestIdle -> SelectDr(1) -> CaptureDr(0) -> ShiftDr(0): 3 bits, TMS = 0b001 = 0x01
        self.shift(3, &[0x01], &[0x00])?;
        // Shift 32 bits: bit 31 has TMS=1 (Exit1Dr)
        let tms = [0x00, 0x00, 0x00, 0x80];
        let tdi = tdi_word.to_le_bytes();
        let tdo = self.shift(32, &tms, &tdi)?;
        // Exit1Dr -> UpdateDr(1) -> RunTestIdle(0): 2 bits, TMS = 0x01
        self.shift(2, &[0x01], &[0x00])?;
        Ok(u32::from_le_bytes([tdo[0], tdo[1], tdo[2], tdo[3]]))
    }

    /// Shift a 64-bit DR scan from `RunTestIdle` and return captured 64-bit TDO word.
    pub fn shift_dr_u64(&mut self, tdi_word: u64) -> Result<u64, String> {
        self.shift(3, &[0x01], &[0x00])?;
        let tms = [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80];
        let tdi = tdi_word.to_le_bytes();
        let tdo = self.shift(64, &tms, &tdi)?;
        self.shift(2, &[0x01], &[0x00])?;
        Ok(u64::from_le_bytes([
            tdo[0], tdo[1], tdo[2], tdo[3], tdo[4], tdo[5], tdo[6], tdo[7],
        ]))
    }

    /// Read the 32-bit JTAG `IDCODE` over XVC.
    pub fn read_idcode(&mut self) -> Result<u32, String> {
        self.shift_ir(IR_IDCODE)?;
        self.shift_dr_u32(0)
    }

    /// Read the 32-bit Helion `STAT` register over XVC.
    pub fn read_stat(&mut self) -> Result<Stat, String> {
        self.shift_ir(IR_STAT)?;
        let w = self.shift_dr_u32(0)?;
        Ok(Stat {
            crc_err: (w & (1 << Stat::BIT_CRC_ERR)) != 0,
            init: (w & (1 << Stat::BIT_INIT)) != 0,
            gts: (w & (1 << Stat::BIT_GTS)) != 0,
            gsr: (w & (1 << Stat::BIT_GSR)) != 0,
            gwe: (w & (1 << Stat::BIT_GWE)) != 0,
            done: (w & (1 << Stat::BIT_DONE)) != 0,
            eos: (w & (1 << Stat::BIT_EOS)) != 0,
        })
    }

    /// Program a Helion [`Bitstream`] over XVC via `IR_CFG_W` and return the resulting [`Stat`].
    pub fn program_bitstream(&mut self, bits: &Bitstream) -> Result<Stat, String> {
        let pkt = &bits.packets;
        if pkt.is_empty() {
            return Err("xvc program_bitstream: empty packet payload".into());
        }
        self.shift_ir(IR_CFG_W)?;
        // Enter ShiftDr: RunTestIdle -> SelectDr(1) -> CaptureDr(0) -> ShiftDr(0)
        self.shift(3, &[0x01], &[0x00])?;

        // Stream packet bytes in chunks that fit within `max_vector_len`
        let max_chunk_bytes = ((self.max_vector_len as usize) / 2).max(64);
        let total_bytes = pkt.len();
        let mut offset = 0usize;
        while offset < total_bytes {
            let end = (offset + max_chunk_bytes).min(total_bytes);
            let chunk = &pkt[offset..end];
            let is_last = end == total_bytes;
            let nbits = (chunk.len() * 8) as u32;
            let mut tms = vec![0u8; chunk.len()];
            if is_last {
                if let Some(last_b) = tms.last_mut() {
                    *last_b = 0x80; // Exit1Dr on the very last bit
                }
            }
            self.shift(nbits, &tms, chunk)?;
            offset = end;
        }

        // Exit1Dr -> UpdateDr(1) -> RunTestIdle(0)
        self.shift(2, &[0x01], &[0x00])?;
        self.read_stat()
    }
}
