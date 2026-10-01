//! Differential driver. Stdout matches `tools/heatshrink-oracle` (`tools/DRIVER_FORMAT.md`).

use std::env;
use std::io::Write;
use std::process::ExitCode;

use heatshrink_core::{Decoder, Encoder, SinkFull};

const POLL_MORE: i32 = 1;
const FINISH_MORE: i32 = 1;
const OUT_BOUND_MUL: usize = 32;
const OUT_BOUND_ADD: usize = 65536;

struct Fixture {
    alloc: String,
    window: u8,
    lookahead: u8,
    input_buffer: u16,
    sink_chunk: usize,
    poll_chunk: usize,
    payload: Vec<u8>,
}

fn die(msg: &str) -> ! {
    println!("error {msg}");
    std::process::exit(2);
}

fn parse_u64(s: &str, max: u64) -> Option<u64> {
    if s.is_empty() || s.as_bytes()[0] == b'+' || s.as_bytes()[0] == b'-' {
        return None;
    }
    if !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let v: u64 = s.parse().ok()?;
    if v > max {
        None
    } else {
        Some(v)
    }
}

fn load_fixture(path: &str) -> Fixture {
    let buf = std::fs::read(path).unwrap_or_else(|_| die("open"));
    let mut pos = 0usize;
    let mut next_line = || -> Option<String> {
        if pos >= buf.len() {
            return None;
        }
        let start = pos;
        while pos < buf.len() && buf[pos] != b'\n' {
            if buf[pos] == b'\r' {
                return None;
            }
            pos += 1;
            if pos - start > 250 {
                return None;
            }
        }
        if pos >= buf.len() || buf[pos] != b'\n' {
            return None;
        }
        let line = String::from_utf8_lossy(&buf[start..pos]).into_owned();
        pos += 1;
        Some(line)
    };
    let magic = next_line().unwrap_or_else(|| die("bad_fixture"));
    if magic != "HSF1" {
        die("bad_fixture");
    }
    let mut alloc = None;
    let mut window = None;
    let mut lookahead = None;
    let mut ibs = None;
    let mut sink_chunk = None;
    let mut poll_chunk = None;
    loop {
        let line = next_line().unwrap_or_else(|| die("bad_fixture"));
        if line.is_empty() {
            break;
        }
        let (key, val) = line.split_once('=').unwrap_or_else(|| die("bad_fixture"));
        match key {
            "alloc" => {
                if alloc.is_some() || (val != "dyn" && val != "static") {
                    die("bad_fixture");
                }
                alloc = Some(val.to_string());
            }
            "window" => {
                if window.is_some() {
                    die("bad_fixture");
                }
                window = Some(parse_u64(val, 255).unwrap_or_else(|| die("bad_fixture")) as u8);
            }
            "lookahead" => {
                if lookahead.is_some() {
                    die("bad_fixture");
                }
                lookahead = Some(parse_u64(val, 255).unwrap_or_else(|| die("bad_fixture")) as u8);
            }
            "input_buffer" => {
                if ibs.is_some() {
                    die("bad_fixture");
                }
                ibs = Some(parse_u64(val, 65535).unwrap_or_else(|| die("bad_fixture")) as u16);
            }
            "sink_chunk" => {
                if sink_chunk.is_some() {
                    die("bad_fixture");
                }
                sink_chunk = Some(
                    parse_u64(val, 0xffff_ffff).unwrap_or_else(|| die("bad_fixture")) as usize,
                );
            }
            "poll_chunk" => {
                if poll_chunk.is_some() {
                    die("bad_fixture");
                }
                poll_chunk = Some(
                    parse_u64(val, 0xffff_ffff).unwrap_or_else(|| die("bad_fixture")) as usize,
                );
            }
            _ => die("bad_fixture"),
        }
    }
    let (
        Some(alloc),
        Some(window),
        Some(lookahead),
        Some(input_buffer),
        Some(sink_chunk),
        Some(poll_chunk),
    ) = (alloc, window, lookahead, ibs, sink_chunk, poll_chunk)
    else {
        die("bad_fixture");
    };
    if alloc == "static" && (window != 8 || lookahead != 4 || input_buffer != 32) {
        die("static_config");
    }
    Fixture {
        alloc,
        window,
        lookahead,
        input_buffer,
        sink_chunk,
        poll_chunk,
        payload: buf[pos..].to_vec(),
    }
}

fn print_hex(label: &str, data: &[u8]) {
    let mut out = std::io::stdout().lock();
    let _ = write!(out, "{label} ");
    if data.is_empty() {
        let _ = writeln!(out, "-");
        return;
    }
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut tmp = [0u8; 4096];
    let mut k = 0;
    for &b in data {
        tmp[k] = HEX[(b >> 4) as usize];
        tmp[k + 1] = HEX[(b & 0x0f) as usize];
        k += 2;
        if k == tmp.len() {
            let _ = out.write_all(&tmp);
            k = 0;
        }
    }
    if k > 0 {
        let _ = out.write_all(&tmp[..k]);
    }
    let _ = writeln!(out);
}

fn over_bound(produced: usize, in_len: usize) -> bool {
    in_len
        .checked_mul(OUT_BOUND_MUL)
        .and_then(|v| v.checked_add(OUT_BOUND_ADD))
        .map(|limit| produced > limit)
        .unwrap_or(false)
}

fn ensure_cap(buf: &mut Vec<u8>, need: usize) {
    if need <= buf.len() {
        return;
    }
    let mut ncap = if buf.is_empty() { 8 } else { buf.len() };
    while ncap < need {
        ncap = ncap.saturating_mul(2).max(need);
        if ncap == need {
            break;
        }
    }
    buf.resize(ncap, 0);
}

enum Machine<'a> {
    Enc(&'a mut Encoder),
    Dec(&'a mut Decoder),
}

impl Machine<'_> {
    fn sink(&mut self, input: &[u8]) -> (i32, usize) {
        match self {
            Machine::Enc(enc) => match enc.sink(input) {
                Ok(n) => (0, n),
                Err(_) => (-2, 0),
            },
            Machine::Dec(dec) => match dec.sink(input) {
                Ok(n) => (0, n),
                Err(SinkFull) => (1, 0),
            },
        }
    }

    fn poll(&mut self, out: &mut [u8]) -> (i32, usize) {
        let result = match self {
            Machine::Enc(enc) => enc.poll(out),
            Machine::Dec(dec) => dec.poll(out),
        };
        match result {
            Ok(p) => (if p.more { POLL_MORE } else { 0 }, p.n),
            Err(_) => (-2, 0),
        }
    }

    fn finish(&mut self) -> i32 {
        let done = match self {
            Machine::Enc(enc) => enc.finish(),
            Machine::Dec(dec) => dec.finish(),
        };
        if done {
            0
        } else {
            FINISH_MORE
        }
    }
}

fn poll_until(
    machine: &mut Machine<'_>,
    buf: &mut Vec<u8>,
    polled: &mut usize,
    poll_chunk: usize,
    in_len: usize,
) -> Option<&'static str> {
    loop {
        if *polled == buf.len() {
            ensure_cap(buf, buf.len() + 1);
        }
        let mut space = buf.len() - *polled;
        if poll_chunk != 0 && space > poll_chunk {
            space = poll_chunk;
        }
        let (pres, got) = {
            let dest = &mut buf[*polled..*polled + space];
            machine.poll(dest)
        };
        println!("op poll {pres} {got}");
        *polled += got;
        if pres < 0 {
            return Some("error");
        }
        if over_bound(*polled, in_len) {
            return Some("bound");
        }
        if pres != POLL_MORE {
            return None;
        }
        if got == 0 {
            return Some("stall");
        }
    }
}

fn run_stream(
    machine: &mut Machine<'_>,
    input: &[u8],
    sink_chunk: usize,
    poll_chunk: usize,
) -> (&'static str, Vec<u8>, usize) {
    let mut cap = input.len() + input.len() / 2 + 4;
    if cap < 4 {
        cap = 4;
    }
    let mut buf = vec![0u8; cap];
    let mut sunk = 0usize;
    let mut polled = 0usize;
    while sunk < input.len() {
        let mut want = input.len() - sunk;
        if sink_chunk != 0 && want > sink_chunk {
            want = sink_chunk;
        }
        let (sres, copied) = machine.sink(&input[sunk..sunk + want]);
        println!("op sink {sres} {copied}");
        if sres < 0 {
            return ("error", buf, polled);
        }
        let sunk_before = sunk;
        let polled_before = polled;
        sunk += copied;
        if sunk == input.len() {
            let fres = machine.finish();
            println!("op finish {fres}");
        }
        if let Some(status) = poll_until(machine, &mut buf, &mut polled, poll_chunk, input.len()) {
            return (status, buf, polled);
        }
        if sunk == input.len() {
            let mut fres = machine.finish();
            println!("op finish {fres}");
            while fres == FINISH_MORE {
                let before = polled;
                if let Some(status) =
                    poll_until(machine, &mut buf, &mut polled, poll_chunk, input.len())
                {
                    return (status, buf, polled);
                }
                fres = machine.finish();
                println!("op finish {fres}");
                if polled == before {
                    return ("stall", buf, polled);
                }
            }
        }
        if sunk == sunk_before && polled == polled_before {
            return ("stall", buf, polled);
        }
    }
    ("ok", buf, polled)
}

fn print_cfg(fx: &Fixture) {
    println!(
        "cfg alloc={} window={} lookahead={} input_buffer={} sink_chunk={} poll_chunk={} in_len={}",
        fx.alloc,
        fx.window,
        fx.lookahead,
        fx.input_buffer,
        fx.sink_chunk,
        fx.poll_chunk,
        fx.payload.len()
    );
}

fn emit_body(status: &str, bytes: &[u8]) {
    println!("status {status}");
    println!("nbytes {}", bytes.len());
    print_hex("hex", bytes);
}

fn section_encode(fx: &Fixture) {
    println!("SECTION encode");
    print_cfg(fx);
    match Encoder::try_new(fx.window, fx.lookahead) {
        Err(_) => {
            println!("alloc null");
            emit_body("alloc_null", &[]);
        }
        Ok(mut enc) => {
            println!("alloc ok");
            let mut machine = Machine::Enc(&mut enc);
            let (status, buf, n) =
                run_stream(&mut machine, &fx.payload, fx.sink_chunk, fx.poll_chunk);
            emit_body(status, &buf[..n]);
        }
    }
    println!("END encode");
}

fn section_decode(fx: &Fixture) {
    println!("SECTION decode");
    print_cfg(fx);
    match Decoder::try_new(fx.input_buffer, fx.window, fx.lookahead) {
        Err(_) => {
            println!("alloc null");
            emit_body("alloc_null", &[]);
        }
        Ok(mut dec) => {
            println!("alloc ok");
            let mut machine = Machine::Dec(&mut dec);
            let (status, buf, n) =
                run_stream(&mut machine, &fx.payload, fx.sink_chunk, fx.poll_chunk);
            emit_body(status, &buf[..n]);
        }
    }
    println!("END decode");
}

fn section_roundtrip(fx: &Fixture) {
    println!("SECTION roundtrip");
    print_cfg(fx);
    let mut comp: Vec<u8> = Vec::new();
    let mut comp_len = 0usize;
    let enc_status;
    let enc_ok;
    match Encoder::try_new(fx.window, fx.lookahead) {
        Err(_) => {
            enc_ok = false;
            enc_status = "alloc_null";
            println!("enc_alloc null");
        }
        Ok(mut enc) => {
            enc_ok = true;
            println!("enc_alloc ok");
            let mut machine = Machine::Enc(&mut enc);
            let (status, buf, n) =
                run_stream(&mut machine, &fx.payload, fx.sink_chunk, fx.poll_chunk);
            enc_status = status;
            comp = buf;
            comp_len = n;
        }
    }
    emit_body(enc_status, if enc_ok { &comp[..comp_len] } else { &[] });

    let mut plain: Vec<u8> = Vec::new();
    let mut plain_len = 0usize;
    let mut dec_ok = false;
    let mut dec_status = "alloc_null";
    if enc_ok && enc_status == "ok" {
        if let Ok(mut dec) = Decoder::try_new(fx.input_buffer, fx.window, fx.lookahead) {
            dec_ok = true;
            println!("dec_alloc ok");
            let mut machine = Machine::Dec(&mut dec);
            let (status, buf, n) = run_stream(
                &mut machine,
                &comp[..comp_len],
                fx.sink_chunk,
                fx.poll_chunk,
            );
            dec_status = status;
            plain = buf;
            plain_len = n;
        } else {
            println!("dec_alloc null");
        }
    } else {
        println!("dec_alloc null");
    }
    emit_body(dec_status, if dec_ok { &plain[..plain_len] } else { &[] });
    let match_ok = dec_ok && dec_status == "ok" && plain[..plain_len] == fx.payload[..];
    println!("match {}", if match_ok { 1 } else { 0 });
    println!("END roundtrip");
}

fn set_sections(list: &str, flags: &mut [bool; 3]) -> bool {
    if list.is_empty() {
        return false;
    }
    flags.fill(false);
    for tok in list.split(',') {
        match tok {
            "encode" => flags[0] = true,
            "decode" => flags[1] = true,
            "roundtrip" => flags[2] = true,
            _ => return false,
        }
    }
    flags.iter().any(|f| *f) && !list.ends_with(',')
}

fn main() -> ExitCode {
    let mut path: Option<String> = None;
    let mut flags = [true, true, true];
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--sections" {
            let Some(list) = args.next() else {
                die("usage")
            };
            if !set_sections(&list, &mut flags) {
                die("usage");
            }
        } else if let Some(list) = arg.strip_prefix("--sections=") {
            if !set_sections(list, &mut flags) {
                die("usage");
            }
        } else if arg.starts_with('-') || path.is_some() {
            die("usage");
        } else {
            path = Some(arg);
        }
    }
    let Some(path) = path else { die("usage") };
    let fx = load_fixture(&path);
    if flags[0] {
        section_encode(&fx);
    }
    if flags[1] {
        section_decode(&fx);
    }
    if flags[2] {
        section_roundtrip(&fx);
    }
    ExitCode::SUCCESS
}
