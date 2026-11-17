//! The text of a crash report.
//!
//! Written after the crash by the module's own thread into a buffer made
//! when the game started: nothing here allocates (unless a report outgrows
//! the buffer) and every read of the process's memory is checked first.
//! The sections and their `key: value` lines are read back by
//! [`doctor_toolkit_live::crash::Report`].

use std::fmt::Write;

use doctor_toolkit_live::addr;
use doctor_toolkit_live::crash::{self, SYMBOLS};
use doctor_toolkit_live::edition::{IMAGE_BASE, NAME};

use crate::hooks::{self, Counted, DISTINCT, ERRORS, RECENT, Sent};
use crate::win::{self, Context, ExceptionRecord, Mem};

/// A loaded module.
#[derive(Clone, Copy)]
pub struct Module {
    pub base: u32,
    pub size: u32,
    name: [u8; 40],
    len: usize,
}

impl Module {
    pub fn name(&self) -> &str {
        std::str::from_utf8(&self.name[..self.len]).unwrap_or("?")
    }
}

/// The modules of the process, read from the loader's list of the PEB
/// (no call of the loader: it may be what crashed).
pub struct Modules {
    list: [Module; Self::MAX],
    count: usize,
}

impl Modules {
    const MAX: usize = 160;

    pub fn read(m: &mut Mem) -> Self {
        let mut out = Self {
            list: [Module {
                base: 0,
                size: 0,
                name: [0; 40],
                len: 0,
            }; Self::MAX],
            count: 0,
        };
        // PEB+0xC: PEB_LDR_DATA; +0x14: InMemoryOrderModuleList. A link is
        // 8 bytes into LDR_DATA_TABLE_ENTRY: DllBase +0x10, SizeOfImage
        // +0x18, BaseDllName (length, buffer) +0x24.
        let Some(ldr) = m.ptr(win::peb() + 0xC) else {
            return out;
        };
        let head = ldr + 0x14;
        let mut link = m.ptr(head).unwrap_or(head);
        while link != head && out.count < Self::MAX {
            let (Some(base), Some(size)) = (m.u32(link + 0x10), m.u32(link + 0x18)) else {
                break;
            };
            let module = &mut out.list[out.count];
            module.base = base;
            module.size = size;
            let chars = usize::from(m.u16(link + 0x24).unwrap_or(0) / 2);
            if let Some(buffer) = m.ptr(link + 0x28) {
                for i in 0..chars {
                    let Some(c) = m.u16(buffer + i as u32 * 2) else {
                        break;
                    };
                    let c = char::from_u32(u32::from(c)).unwrap_or('?');
                    let mut utf8 = [0u8; 4];
                    let bytes = c.encode_utf8(&mut utf8).as_bytes();
                    if module.len + bytes.len() > module.name.len() {
                        break;
                    }
                    module.name[module.len..module.len + bytes.len()].copy_from_slice(bytes);
                    module.len += bytes.len();
                }
            }
            if base != 0 {
                out.count += 1;
            }
            let Some(next) = m.ptr(link) else { break };
            link = next;
        }
        out
    }

    pub fn all(&self) -> &[Module] {
        &self.list[..self.count]
    }

    pub fn of(&self, address: u32) -> Option<&Module> {
        self.all()
            .iter()
            .find(|m| address >= m.base && address - m.base < m.size)
    }
}

/// What the module knew before the crash.
pub struct Startup<'a> {
    /// `doctor_crash 0.0.6`.
    pub version: &'a str,
    /// `GetTickCount` when the module loaded.
    pub loaded_tick: u32,
    /// [`doctor_toolkit_live::crash::CONTEXT_FILE`] as the toolkit wrote it.
    pub context: &'a str,
    /// What of the hooks could not be installed.
    pub hooks_failed: &'a str,
    /// The first line of `cmd.txt` of the game folder when the game
    /// started (its launch options), without line ends.
    pub cmd_txt: &'a str,
}

/// The crash.
pub struct Crash<'a> {
    pub record: &'a ExceptionRecord,
    pub context: &'a Context,
    pub thread: u32,
    pub utc: win::SystemTime,
    pub local: win::SystemTime,
    pub tick: u32,
}

fn image(m: &mut Mem) -> impl FnMut(u32, &mut [u8]) -> bool + '_ {
    |va, out| m.bytes(va, out)
}

/// `module+0xOFFSET`, or that the address lies in no module.
fn place(out: &mut String, modules: &Modules, address: u32) {
    match modules.of(address) {
        Some(m) => {
            let _ = write!(out, "{}+{:#010x}", m.name(), address - m.base);
        }
        None => out.push_str("(in no module)"),
    }
}

/// The start of a description: lines of the stack and of the messages
/// stay readable; the whole text is in the knowledge base (and in
/// `[exception]` for the crash address).
fn short(out: &mut String, what: &str) {
    const MAX: usize = 72;
    for (i, c) in what.chars().enumerate() {
        if i == MAX {
            out.push('…');
            return;
        }
        out.push(c);
    }
}

/// What the knowledge base says about an address of the executable, on
/// the same line: ` = [note] what` for a named address; for another one
/// the function around it — ` in [note] what (+0xN)` when the knowledge
/// base names its start, else ` in 0xSTART +0xN` with the nearest named
/// address inside it, if there is one.
fn name(out: &mut String, m: &mut Mem, address: u32) {
    if let Some(s) = crash::symbols(SYMBOLS).find(|s| s.address == address) {
        let _ = write!(out, " = [{}] ", s.note);
        short(out, s.what);
        return;
    }
    if !crash::in_text(address) {
        return;
    }
    let Some(start) = crash::function_start(&mut image(m), address) else {
        return;
    };
    match crash::symbol_before(SYMBOLS, address, start) {
        Some(s) if s.address == start => {
            let _ = write!(out, " in [{}] ", s.note);
            short(out, s.what);
            let _ = write!(out, " ({start:#010x} +{:#x})", address - start);
        }
        Some(s) => {
            let _ = write!(
                out,
                " in {start:#010x} +{:#x} (after {:#010x}: [{}] ",
                address - start,
                s.address,
                s.note
            );
            short(out, s.what);
            out.push(')');
        }
        // A function the knowledge base does not name: its start says
        // more than nothing, unless the address is the start itself.
        None if address != start => {
            let _ = write!(out, " in {:#010x} +{:#x}", start, address - start);
        }
        None => {}
    }
}

/// An address with its module and its name.
fn described(out: &mut String, m: &mut Mem, modules: &Modules, address: u32) {
    let _ = write!(out, "{address:#010x} ");
    place(out, modules, address);
    name(out, m, address);
}

fn lossy(out: &mut String, bytes: &[u8]) {
    for &b in bytes {
        out.push(match b {
            b'\t' | b'\r' | b'\n' => ' ',
            0..0x20 | 0x7F => '?',
            _ => char::from(b),
        });
    }
}

fn stamp(out: &mut String, t: &win::SystemTime) {
    let _ = write!(
        out,
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        t.year, t.month, t.day, t.hour, t.minute, t.second
    );
}

/// Seconds before the crash, `-12.345 s`.
fn ago(out: &mut String, crash_tick: u32, tick: u32) {
    let ms = crash_tick.wrapping_sub(tick);
    let _ = write!(out, "-{}.{:03} s", ms / 1000, ms % 1000);
}

fn exception(out: &mut String, m: &mut Mem, modules: &Modules, crash: &Crash) {
    let r = crash.record;
    out.push_str("\n[exception]\n");
    let _ = write!(
        out,
        "code: {:#010X} {}",
        r.code,
        crash::exception_name(r.code)
    );
    if matches!(r.code, 0xC000_0005 | 0xC000_0006) && r.parameters >= 2 {
        let what = match r.information[0] {
            0 => "read",
            1 => "write",
            8 => "execution",
            _ => "access",
        };
        let _ = write!(out, " ({what} of {:#010x})", r.information[1]);
    }
    out.push('\n');
    let _ = write!(out, "address: {:#010x} ", r.address);
    place(out, modules, r.address);
    out.push('\n');
    for s in crash::symbols(SYMBOLS).filter(|s| s.address == r.address) {
        let _ = writeln!(out, "known: [{}] {}", s.note, s.what);
    }
    if crash::in_text(r.address) && modules.of(r.address).is_some_and(|m| m.base == IMAGE_BASE) {
        match crash::function_start(&mut image(m), r.address) {
            Some(start) => {
                let _ = write!(out, "function: {start:#010x} +{:#x}", r.address - start);
                match crash::symbol_before(SYMBOLS, r.address, start) {
                    Some(s) => {
                        let _ = writeln!(
                            out,
                            "; the knowledge base names {:#010x} in it: [{}] {}",
                            s.address, s.note, s.what
                        );
                    }
                    None => out.push_str("; the knowledge base has no name in it\n"),
                }
            }
            None => out.push_str("function: start not found\n"),
        }
    } else if modules.of(r.address).is_none() {
        memory(out, r.address);
        out.push_str("function: none — the address lies in no module");
        if let Some(text) = crash::as_text(r.address) {
            out.push_str("; its bytes read as the text \"");
            lossy(out, &text);
            out.push_str(
                "\": the processor jumped through a pointer overwritten with text (see the first return address of [stack])",
            );
        }
        out.push('\n');
    }
    let _ = writeln!(out, "thread: {:#x}", crash.thread);
    let _ = writeln!(out, "flags: {:#x}", r.flags);
}

/// What the system says about the memory at an address outside every
/// module.
fn memory(out: &mut String, address: u32) {
    let Some(info) = win::query(address) else {
        out.push_str("memory: the system knows nothing about this address\n");
        return;
    };
    let _ = writeln!(
        out,
        "memory: {} region {:#010x}-{:#010x}, allocation base {:#010x}, protection {:#x}, {}",
        match info.state {
            win::MEM_COMMIT => "committed",
            0x2000 => "reserved",
            _ => "free",
        },
        info.base_address,
        info.base_address.wrapping_add(info.region_size),
        info.allocation_base,
        info.protect,
        match info.kind {
            0x0100_0000 => "an image",
            0x0004_0000 => "a mapped file",
            0x0002_0000 => "private memory",
            _ => "no type",
        }
    );
}

fn registers(out: &mut String, m: &mut Mem, modules: &Modules, c: &Context) {
    out.push_str("\n[registers]\n");
    let w = &c.words;
    let _ = writeln!(
        out,
        "eax={:08x} ebx={:08x} ecx={:08x} edx={:08x} esi={:08x} edi={:08x}",
        w[Context::EAX],
        w[Context::EBX],
        w[Context::ECX],
        w[Context::EDX],
        w[Context::ESI],
        w[Context::EDI]
    );
    let _ = writeln!(
        out,
        "eip={:08x} esp={:08x} ebp={:08x} eflags={:08x}",
        w[Context::EIP],
        w[Context::ESP],
        w[Context::EBP],
        w[Context::EFLAGS]
    );
    let _ = writeln!(
        out,
        "cs={:04x} ss={:04x} ds={:04x} es={:04x} fs={:04x} gs={:04x}",
        w[Context::CS],
        w[Context::SS],
        w[Context::DS],
        w[Context::ES],
        w[Context::FS],
        w[Context::GS]
    );
    // What the registers point at, where that says something.
    for (reg, value) in [
        ("eax", w[Context::EAX]),
        ("ebx", w[Context::EBX]),
        ("ecx", w[Context::ECX]),
        ("edx", w[Context::EDX]),
        ("esi", w[Context::ESI]),
        ("edi", w[Context::EDI]),
    ] {
        if modules.of(value).is_some() {
            let _ = write!(out, "{reg} -> ");
            described(out, m, modules, value);
            out.push('\n');
        } else if let Some(first) = m.u32(value)
            && modules.of(first).is_some_and(|m| m.base == IMAGE_BASE)
        {
            // An object: its first word is a table of the executable.
            let _ = write!(out, "{reg} -> an object, table ");
            described(out, m, modules, first);
            out.push('\n');
        }
    }
    // The memory the registers point at outside the modules (a heap
    // object, a request of a queue): what a pointer was overwritten with
    // is often readable right there.
    let stack_low = w[Context::ESP] & !0xFFF;
    for (reg, value) in [
        ("eax", w[Context::EAX]),
        ("ebx", w[Context::EBX]),
        ("ecx", w[Context::ECX]),
        ("edx", w[Context::EDX]),
        ("esi", w[Context::ESI]),
        ("edi", w[Context::EDI]),
        ("ebp", w[Context::EBP]),
    ] {
        let at = value & !3;
        if modules.of(value).is_some()
            || (stack_low..stack_low.wrapping_add(0x10000)).contains(&value)
        {
            continue;
        }
        let mut data = [0u8; 0x60];
        if !m.bytes(at, &mut data) {
            continue;
        }
        let _ = write!(out, "memory at {reg} {at:08x}:");
        for (i, b) in data.iter().enumerate() {
            if i % 4 == 0 {
                out.push(' ');
            }
            let _ = write!(out, "{b:02X}");
        }
        out.push('\n');
    }
    let mut code = [0u8; 16];
    if m.bytes(w[Context::EIP], &mut code) {
        out.push_str("code at eip:");
        for b in code {
            let _ = write!(out, " {b:02X}");
        }
        out.push('\n');
    } else {
        out.push_str("code at eip: cannot be read\n");
    }
}

/// The target of a call that returns to `ret`, when the bytes before it
/// are a call: `Some(Some(target))` for a direct one, `Some(None)` for an
/// indirect one.
fn call_before(m: &mut Mem, ret: u32) -> Option<Option<u32>> {
    let mut b = [0u8; 7];
    if ret < 0x1_0007 || !m.bytes(ret - 7, &mut b) {
        return None;
    }
    if b[2] == 0xE8 {
        let rel = u32::from_le_bytes([b[3], b[4], b[5], b[6]]);
        return Some(Some(ret.wrapping_add(rel)));
    }
    // FF /2: `call r/m32` of 2, 3, 6 or 7 bytes.
    let indirect = |opcode: u8, modrm: u8| opcode == 0xFF && modrm & 0x38 == 0x10;
    let found = (indirect(b[5], b[6]) && b[6] >> 6 == 3)
        || (indirect(b[5], b[6]) && b[6] >> 6 == 0 && b[6] & 7 != 4 && b[6] & 7 != 5)
        || (indirect(b[4], b[5]) && (b[5] >> 6 == 1 || (b[5] >> 6 == 0 && b[5] & 7 == 4)))
        || (indirect(b[3], b[4]) && b[4] >> 6 == 1 && b[4] & 7 == 4)
        || (indirect(b[1], b[2]) && (b[2] >> 6 == 2 || (b[2] >> 6 == 0 && b[2] & 7 == 5)))
        || (indirect(b[0], b[1]) && b[1] >> 6 == 2 && b[1] & 7 == 4);
    found.then_some(None)
}

fn stack(out: &mut String, m: &mut Mem, modules: &Modules, c: &Context) {
    const WORDS: u32 = 2048;
    const LINES: usize = 64;
    let esp = c.words[Context::ESP];
    out.push_str("\n[stack]\n");
    out.push_str(
        "# Words of the stack that are return addresses (they follow a call instruction), from the top.\n",
    );
    let mut lines = 0;
    let mut read = 0;
    for i in 0..WORDS {
        let Some(value) = m.u32(esp.wrapping_add(i * 4)) else {
            break;
        };
        read = i + 1;
        if modules.of(value).is_none() || !m.executable(value) {
            continue;
        }
        let Some(target) = call_before(m, value) else {
            continue;
        };
        let _ = write!(out, "esp+{:#06x} ", i * 4);
        described(out, m, modules, value);
        if let Some(target) = target {
            let _ = write!(out, "; after call {target:#010x}");
            name(out, m, target);
        }
        out.push('\n');
        lines += 1;
        if lines == LINES {
            out.push_str("# (more below: see the minidump)\n");
            break;
        }
    }
    if read == 0 {
        let _ = writeln!(out, "# The stack at {esp:#010x} cannot be read.");
    }

    out.push_str("\n[frames]\n");
    out.push_str(
        "# The chain of EBP; right only where the code keeps frame pointers (most of the game does not).\n",
    );
    let mut frame = c.words[Context::EBP];
    for _ in 0..32 {
        if frame < esp || frame - esp > 0x10_0000 || !frame.is_multiple_of(4) {
            break;
        }
        let (Some(next), Some(ret)) = (m.u32(frame), m.u32(frame + 4)) else {
            break;
        };
        if modules.of(ret).is_none() {
            break;
        }
        let _ = write!(out, "ebp {frame:#010x} ");
        described(out, m, modules, ret);
        out.push('\n');
        if next <= frame {
            break;
        }
        frame = next;
    }
}

/// The words of the command line after the program.
fn command_line(out: &mut String, m: &mut Mem) {
    // SAFETY: returns a pointer into the process's parameters.
    let line = unsafe { win::GetCommandLineW() } as usize as u32;
    let mut quoted = false;
    let mut program = true;
    for i in 0..2048u32 {
        let Some(c) = m.u16(line + i * 2).filter(|&c| c != 0) else {
            break;
        };
        let c = char::from_u32(u32::from(c)).unwrap_or('?');
        if program {
            // The path of the program is left out: it names the person's
            // folders.
            match c {
                '"' => quoted = !quoted,
                ' ' | '\t' if !quoted => program = false,
                _ => {}
            }
            continue;
        }
        out.push(if c.is_control() { ' ' } else { c });
    }
}

fn game(out: &mut String, m: &mut Mem, crash: &Crash, startup: &Startup) {
    out.push_str("\n[game]\n");
    out.push_str("arguments: ");
    command_line(out, m);
    out.push('\n');
    // The game takes its options from cmd.txt, not from the command line.
    let _ = writeln!(
        out,
        "cmd.txt: {}",
        if startup.cmd_txt.is_empty() {
            "(none in the game folder when the game started)"
        } else {
            startup.cmd_txt
        }
    );
    let up = crash.tick.wrapping_sub(startup.loaded_tick);
    let _ = writeln!(out, "running for: {}.{:03} s", up / 1000, up % 1000);
    let state = m
        .ptr(addr::LOOP_DISPATCHER)
        .and_then(|d| m.ptr(d + 4))
        .and_then(|s| m.u32(s));
    let _ = writeln!(
        out,
        "main loop: {}",
        match state {
            Some(addr::VTABLE_INIT_LOOP) => "start-up (InitOnceMainLoop)",
            Some(addr::VTABLE_FRONTEND_LOOP) => "menu (FrontendMainLoop)",
            Some(addr::VTABLE_GAME_LOOP) => "level (GameMainLoop)",
            Some(_) => "another state",
            None => "none yet",
        }
    );
    if let Some(ms) = m.u32(addr::SIM_TIME_MS) {
        let _ = writeln!(out, "game time: {}.{:03} s", ms / 1000, ms % 1000);
    }
    let player = m
        .ptr(addr::PLAYER_LIST)
        .filter(|&list| m.u32(list + 0xC).is_some_and(|n| n != 0))
        .and_then(|list| m.ptr(list + 4))
        .and_then(|array| m.ptr(array))
        .and_then(|entry| entry.checked_sub(addr::PLAYER_ENTRY_OFFSET));
    match player {
        Some(p) => {
            let at = p + addr::ENTITY_MATRIX + 48;
            match (m.f32(at), m.f32(at + 4), m.f32(at + 8)) {
                (Some(x), Some(y), Some(z)) => {
                    let _ = writeln!(out, "player: {x:.2} {y:.2} {z:.2}");
                }
                _ => out.push_str("player: cannot be read\n"),
            }
        }
        None => out.push_str("player: none\n"),
    }
}

fn lua_errors(out: &mut String, crash: &Crash) {
    out.push_str("\n[lua errors]\n");
    if hooks::INSTALLED.load(std::sync::atomic::Ordering::Relaxed) & 2 == 0 {
        out.push_str("# Not recorded: luaD_throw is not hooked.\n");
        return;
    }
    let done = hooks::LUA.with(5_000_000, |errors| {
        let _ = writeln!(
            out,
            "# {} error(s) of Lua since the game started (those a script caught itself too); the last {ERRORS}, newest first.",
            errors.total
        );
        for i in 0..ERRORS {
            let slot = (errors.next + ERRORS - 1 - i) % ERRORS;
            let Some(e) = &errors.list[slot] else {
                continue;
            };
            ago(out, crash.tick, e.tick);
            let _ = write!(
                out,
                "  thread {:#x}  {}",
                e.thread,
                match e.code {
                    1 => "run",
                    3 => "syntax",
                    4 => "memory",
                    5 => "in the error handler",
                    _ => "error",
                }
            );
            if e.count > 1 {
                let _ = write!(out, "  x{}", e.count);
            }
            out.push_str("  ");
            lossy(out, &e.text[..e.len]);
            out.push('\n');
        }
    });
    if done.is_none() {
        out.push_str("# Not read: the table was busy (the crash came inside its hook).\n");
    }
}

fn sent(out: &mut String, m: &mut Mem, modules: &Modules, crash: &Crash, s: &Sent, count: u32) {
    ago(out, crash.tick, s.tick);
    let _ = write!(out, "  #{}  {:#010x}", s.seq, s.hash);
    if let Some(n) = crash::engine_message(s.hash) {
        let _ = write!(out, " {n}");
    }
    if count > 1 {
        let _ = write!(out, "  x{count}");
    }
    let _ = write!(
        out,
        "  data {:#010x}  thread {:#x}  from ",
        s.data, s.thread
    );
    described(out, m, modules, s.caller);
    out.push('\n');
}

fn messages(out: &mut String, m: &mut Mem, modules: &Modules, crash: &Crash) {
    out.push_str("\n[messages]\n");
    if hooks::INSTALLED.load(std::sync::atomic::Ordering::Relaxed) & 1 == 0 {
        out.push_str("# Not recorded: the message bus is not hooked.\n");
        return;
    }
    // Copied out first: naming the senders takes time, and the game's
    // other threads still send.
    let mut recent = [Sent::default(); RECENT];
    let mut distinct = [Counted::default(); DISTINCT];
    let Some(total) = hooks::BUS.with(5_000_000, |bus| {
        recent = bus.recent;
        distinct = bus.distinct;
        bus.seq
    }) else {
        out.push_str("# Not read: the table was busy (the crash came inside its hook).\n");
        return;
    };
    let _ = writeln!(
        out,
        "# {total} message(s) sent through the bus since the game started. Time before the crash, number, hash and name (engine messages), data, thread, sender (where Send returns to)."
    );
    let _ = writeln!(out, "# The last {RECENT} in order, newest first:");
    recent.sort_unstable_by_key(|s| std::cmp::Reverse(s.seq));
    for s in recent.iter().filter(|s| s.seq != 0) {
        sent(out, m, modules, crash, s, 1);
    }
    out.push_str(
        "# Each message once, by its last send, newest first (xN: times sent); the per-frame ticks do not push the rare ones out:\n",
    );
    distinct.sort_unstable_by_key(|c| std::cmp::Reverse(c.last.seq));
    for c in distinct.iter().filter(|c| c.count != 0).take(96) {
        sent(out, m, modules, crash, &c.last, c.count);
    }
}

/// Name of a stream record: pointer `+8`, length `+0xC`.
fn stream_name(out: &mut String, m: &mut Mem, record: u32) {
    let len = m.u32(record + 0xC).unwrap_or(0).min(200) as usize;
    let mut buf = [0u8; 200];
    match m.ptr(record + 8) {
        Some(p) if len > 0 && m.bytes(p, &mut buf[..len]) => {
            let end = buf[..len].iter().position(|&b| b == 0).unwrap_or(len);
            lossy(out, &buf[..end]);
        }
        _ => {
            let _ = write!(out, "? (record {record:#010x})");
        }
    }
}

fn streams(out: &mut String, m: &mut Mem) {
    out.push_str("\n[streams]\n");
    let Some(manager) = m.ptr(addr::STREAM_MANAGER) else {
        out.push_str("# No StreamManager yet.\n");
        return;
    };
    out.push_str("making objects of: ");
    match m.ptr(manager + addr::STREAM_LOADING) {
        Some(record) => stream_name(out, m, record),
        None => out.push_str("(nothing)"),
    }
    out.push('\n');
    // Records by id: buckets +0x50, their count +0x54; a node is {id,
    // record, next}; the state of a record is at +0x24, 3 — loaded.
    out.push_str("# Asked for and not loaded yet (state of the record: 3 would be loaded):\n");
    let buckets = m.ptr(manager + 0x50);
    let count = m.u32(manager + 0x54).unwrap_or(0).min(8192);
    let mut waiting = 0;
    let mut records = 0;
    if let Some(buckets) = buckets {
        'all: for b in 0..count {
            let mut node = m.u32(buckets + b * 4).unwrap_or(0);
            let mut chain = 0;
            while node >= 0x1_0000 && chain < 256 {
                chain += 1;
                records += 1;
                let (Some(id), Some(record)) = (m.u32(node), m.ptr(node + 4)) else {
                    break;
                };
                let state = m.u32(record + addr::STREAM_RECORD_STATE).unwrap_or(0);
                if state != 3 {
                    waiting += 1;
                    if waiting > 200 {
                        out.push_str("# (more)\n");
                        break 'all;
                    }
                    let _ = write!(out, "waiting: state {state}  id {id:#010x}  ");
                    stream_name(out, m, record);
                    out.push('\n');
                }
                node = m.u32(node + 8).unwrap_or(0);
            }
        }
    }
    let _ = writeln!(out, "records: {records}, not loaded: {waiting}");
    out.push_str("# Loaded, in the manager's order (the zones of the world are streams too):\n");
    let mut node = m.u32(manager + addr::STREAM_LIST).unwrap_or(0);
    let mut n = 0;
    while node >= 0x1_0000 && n < 1024 {
        n += 1;
        out.push_str("loaded: ");
        stream_name(out, m, node);
        out.push('\n');
        node = m.u32(node + 4).unwrap_or(0);
    }
    let _ = writeln!(out, "loaded streams: {n}");
}

fn mods(out: &mut String, startup: &Startup) {
    out.push_str("\n[mods]\n");
    if startup.context.trim().is_empty() {
        out.push_str(
            "# Unknown: no context.txt — the toolkit writes it when mods are enabled and when it starts the game.\n",
        );
        return;
    }
    out.push_str(
        "# As the toolkit wrote before the launch (mod: id, version, name; in load order).\n",
    );
    for line in startup.context.lines() {
        let mut parts = line.trim_end().split('\t');
        let Some(key) = parts.next().filter(|k| !k.is_empty()) else {
            continue;
        };
        let _ = write!(out, "{key}:");
        for (i, part) in parts.enumerate() {
            out.push_str(if i == 0 { " " } else { " | " });
            out.push_str(part);
        }
        out.push('\n');
    }
}

fn modules_section(out: &mut String, modules: &Modules) {
    out.push_str("\n[modules]\n");
    for m in modules.all() {
        let _ = writeln!(
            out,
            "{:#010x}-{:#010x} {}",
            m.base,
            m.base.wrapping_add(m.size),
            m.name()
        );
    }
}

/// Writes the whole report but its last section (the minidump is written
/// after the text is safe on the disk).
pub fn write(out: &mut String, crash: &Crash, startup: &Startup) {
    let mut m = Mem::default();
    let modules = Modules::read(&mut m);
    let _ = writeln!(out, "{} on {NAME}", startup.version);
    out.push_str("time: ");
    stamp(out, &crash.utc);
    out.push_str(" UTC (");
    stamp(out, &crash.local);
    out.push_str(" local)\n");
    if !startup.hooks_failed.is_empty() {
        let _ = writeln!(out, "not hooked: {}", startup.hooks_failed);
    }
    exception(out, &mut m, &modules, crash);
    registers(out, &mut m, &modules, crash.context);
    stack(out, &mut m, &modules, crash.context);
    game(out, &mut m, crash, startup);
    lua_errors(out, crash);
    messages(out, &mut m, &modules, crash);
    streams(out, &mut m);
    mods(out, startup);
    modules_section(out, &modules);
}
