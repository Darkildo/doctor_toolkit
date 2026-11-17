//! Crash reports of the game (stage 99): what the crash module
//! `doctor_crash.asi` (crate `doctor-toolkit-crash-asi`) and the tools that
//! read its reports share.
//!
//! The module is a third, small DLL for the ASI loader, independent of the
//! live module and the graphics module: it works with either, with both and
//! with none (a player who only plays mods). On PC 1.0.789.0 it installs
//! the process's unhandled-exception filter and two hooks of its own —
//! places neither other module checks or patches ([`CHECKS`]): the message
//! bus after its first instructions ([`BUS_HOOK`]) and `luaD_throw`
//! ([`LUA_THROW`]). On a crash it writes a minidump and a text report into
//! `_doctor_toolkit/crashes/` of the game folder — the only folder it
//! writes to. On another build it installs nothing.
//!
//! Here:
//!
//! - the names of its files and the places it checks;
//! - [`symbols`]: the table of named addresses generated from the knowledge
//!   base (`data/gf2/exe_symbols.tsv`, `doctor-toolkit kb-gen`), and the
//!   lookups over it — the module embeds the table, `crash-report` reads
//!   the same one;
//! - [`function_start`]: the start of the function around an address, by
//!   the padding the compiler left between functions;
//! - [`Report`]: the text report, parsed back for the tools.

use crate::edition::Check;

/// File name of the module in the game folder (under `scripts/`).
pub const MODULE_FILE: &str = "doctor_crash.asi";
/// The folder of the reports, from the game folder. The module writes
/// nowhere else.
pub const CRASH_DIR: &str = "_doctor_toolkit/crashes";
/// The module's log, in [`CRASH_DIR`].
pub const LOG_FILE: &str = "doctor_crash.log";
/// What the toolkit knows about the launch — the enabled mods and their
/// versions — in [`CRASH_DIR`]; the module copies it into a report
/// ([`Context`]).
pub const CONTEXT_FILE: &str = "context.txt";
/// A file in [`CRASH_DIR`] that asks the module to crash the game on
/// purpose, for testing (`crash-report --test`); its text is the kind of
/// crash ([`TEST_KINDS`]). The module deletes it first.
pub const TEST_FILE: &str = "crash-now.txt";
/// Kinds of a crash on purpose: a read of address 0x14 from the message
/// bus hook on a thread of the game, a call of an address outside every
/// module (`0x6E756F7B`, the known crash) from the same place, and a read
/// of address 0 on the module's own thread.
pub const TEST_KINDS: [&str; 3] = ["read", "jump", "thread"];
/// Start of the names of a report's files: `crash-YYYYMMDD-HHMMSS.txt` and
/// `.dmp` (UTC).
pub const REPORT_PREFIX: &str = "crash-";
/// Reports the module keeps; older ones go when the game starts.
pub const KEEP_REPORTS: usize = 10;
/// First line of a report starts with this.
pub const REPORT_MAGIC: &str = "doctor_crash ";

/// `.text` of the executable: `[TEXT_START, TEXT_END)`.
pub const TEXT_START: u32 = 0x0040_1000;
pub const TEXT_END: u32 = 0x00D5_435A;
/// End of the image (`IMAGE_BASE + SIZE_OF_IMAGE`).
pub const IMAGE_END: u32 = 0x0123_2000;

/// The module's hook of the message bus: `and esi, 0xFFF` (6 bytes) of
/// `Send` (`0x00408A00`), after the hash went to `ECX` and the message to
/// `EDX`. The live module jumps from the first five bytes of the function
/// and checks its first sixteen; this place lies after them, so each
/// module finds its bytes whichever loads first.
pub const BUS_HOOK: u32 = 0x0040_8A11;
pub const BUS_HOOK_BYTES: [u8; 6] = [0x81, 0xE6, 0xFF, 0x0F, 0x00, 0x00];
/// Where the bus goes on after the hooked instruction.
pub const BUS_HOOK_RETURN: u32 = 0x0040_8A17;
/// `luaD_throw` (`L` in `ECX`, the error code in `EDX`): every error of
/// Lua passes it before the stack unwinds; its first five bytes are
/// `mov eax, [ecx+0x50]; test eax, eax`. The live module hooks the calls of
/// `luaD_rawrunprotected` and `luaG_errormsg` instead.
pub const LUA_THROW: u32 = 0x0063_5A80;
pub const LUA_THROW_BYTES: [u8; 5] = [0x8B, 0x41, 0x50, 0x85, 0xC0];
pub const LUA_THROW_RETURN: u32 = 0x0063_5A85;

/// The places the crash module checks before it installs anything (after
/// the PE header, [`crate::edition::check_with`]).
pub const CHECKS: &[Check] = &[
    Check {
        address: BUS_HOOK,
        what: "message bus: `and esi, 0xFFF; mov esi, [edi+esi*4]` after the hash and the table",
        bytes: &[0x81, 0xE6, 0xFF, 0x0F, 0x00, 0x00, 0x8B, 0x34, 0xB7],
    },
    Check {
        address: LUA_THROW,
        what: "luaD_throw: errorJmp at L+0x50, then longjmp",
        bytes: &[
            0x8B, 0x41, 0x50, 0x85, 0xC0, 0x74, 0x11, 0x89, 0x50, 0x44, 0x8B, 0x41, 0x50, 0x6A,
            0x01, 0x83, 0xC0, 0x04, 0x50, 0xE8, 0xB4, 0xB0, 0x56, 0x00,
        ],
    },
    Check {
        address: 0x0040_5F40,
        what: "StreamManager at 0x0122337C: list +0x6C, next +4, id +0x1C",
        bytes: &[
            0x8B, 0x41, 0x40, 0x8B, 0x0C, 0x81, 0x8B, 0x11, 0x8B, 0x0A, 0x8B, 0x15, 0x7C, 0x33,
            0x22, 0x01, 0x8B, 0x82, 0x80, 0x00, 0x00, 0x00, 0x85, 0xC0, 0x74, 0x09, 0x39, 0x48,
            0x2C, 0x75, 0x04, 0x8B, 0x40, 0x1C, 0xC3, 0x8B, 0x42, 0x6C,
        ],
    },
    Check {
        address: 0x0040_25A0,
        what: "StreamManager records: buckets +0x50, their count +0x54; node {id, record, next}",
        bytes: &[
            0x56, 0x8B, 0x74, 0x24, 0x08, 0x33, 0xD2, 0x8B, 0xC6, 0xF7, 0x71, 0x54, 0x8B, 0x41,
            0x50, 0x57, 0x33, 0xFF, 0x8B, 0x14, 0x90,
        ],
    },
];

// ---------------------------------------------------------------- symbols

/// The table of named addresses, generated from `docs/kb/exe/`.
pub const SYMBOLS: &str = include_str!("../../../data/gf2/exe_symbols.tsv");
/// Names of the engine's messages (`data/gf2/engine_messages.txt`).
const ENGINE_MESSAGES: &str = include_str!("../../../data/gf2/engine_messages.txt");

/// One line of the table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Symbol<'a> {
    pub address: u32,
    /// The note of `docs/kb/exe/` (file name without `.md`).
    pub note: &'a str,
    /// What the note says the address is.
    pub what: &'a str,
}

/// The lines of a table (comments and broken lines skipped), in the order
/// of the file: by address. Allocates nothing — the crash handler uses it.
pub fn symbols(table: &str) -> impl Iterator<Item = Symbol<'_>> {
    table.lines().filter_map(|line| {
        if line.starts_with('#') {
            return None;
        }
        let mut parts = line.splitn(3, '\t');
        let address = u32::from_str_radix(parts.next()?, 16).ok()?;
        Some(Symbol {
            address,
            note: parts.next()?,
            what: parts.next()?,
        })
    })
}

/// Whether an address lies in the code of the executable.
pub fn in_text(address: u32) -> bool {
    (TEXT_START..TEXT_END).contains(&address)
}

/// The nearest named address of the code below `address` (not `address`
/// itself) and no lower than `floor`.
pub fn symbol_before(table: &str, address: u32, floor: u32) -> Option<Symbol<'_>> {
    symbols(table)
        .take_while(|s| s.address < address)
        .filter(|s| s.address >= floor && in_text(s.address))
        .last()
}

/// The start of the function around `address` of the executable's code:
/// the nearest 16-byte boundary at or below it that follows a padding byte
/// `0xCC` (the compiler aligned the functions of this build and filled the
/// gaps with `int3`). `read` copies bytes of the image. `None` outside the
/// code or when no boundary is found in 64 KB. A heuristic: a function that
/// ends exactly at a boundary hides the start of the next one.
pub fn function_start(read: &mut dyn FnMut(u32, &mut [u8]) -> bool, address: u32) -> Option<u32> {
    if !in_text(address) {
        return None;
    }
    let mut at = address & !15;
    let floor = address.saturating_sub(0x1_0000).max(TEXT_START);
    while at > floor {
        let mut b = [0u8; 1];
        if !read(at - 1, &mut b) {
            return None;
        }
        if b[0] == 0xCC {
            return Some(at);
        }
        at -= 16;
    }
    (at == TEXT_START).then_some(TEXT_START)
}

/// The name of an engine message by its hash.
pub fn engine_message(hash: u32) -> Option<&'static str> {
    ENGINE_MESSAGES
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .find(|name| crate::name_hash(name) == hash)
}

/// Name of an exception code.
pub fn exception_name(code: u32) -> &'static str {
    match code {
        0xC000_0005 => "ACCESS_VIOLATION",
        0xC000_0006 => "IN_PAGE_ERROR",
        0xC000_001D => "ILLEGAL_INSTRUCTION",
        0xC000_0025 => "NONCONTINUABLE_EXCEPTION",
        0xC000_0026 => "INVALID_DISPOSITION",
        0xC000_008C => "ARRAY_BOUNDS_EXCEEDED",
        0xC000_008D => "FLT_DENORMAL_OPERAND",
        0xC000_008E => "FLT_DIVIDE_BY_ZERO",
        0xC000_008F => "FLT_INEXACT_RESULT",
        0xC000_0090 => "FLT_INVALID_OPERATION",
        0xC000_0091 => "FLT_OVERFLOW",
        0xC000_0092 => "FLT_STACK_CHECK",
        0xC000_0093 => "FLT_UNDERFLOW",
        0xC000_0094 => "INT_DIVIDE_BY_ZERO",
        0xC000_0095 => "INT_OVERFLOW",
        0xC000_0096 => "PRIVILEGED_INSTRUCTION",
        0xC000_00FD => "STACK_OVERFLOW",
        0xC000_0374 => "HEAP_CORRUPTION",
        0xC000_0409 => "STACK_BUFFER_OVERRUN",
        0x8000_0002 => "DATATYPE_MISALIGNMENT",
        0x8000_0003 => "BREAKPOINT",
        0x8000_0004 => "SINGLE_STEP",
        0xE06D_7363 => "C++ exception",
        _ => "exception",
    }
}

/// An address whose four bytes are printable ASCII, as text in memory
/// order: `0x6E756F7B` is `{oun`. A crash at such an address outside every
/// module means the processor jumped through a pointer overwritten with
/// text.
pub fn as_text(address: u32) -> Option<[u8; 4]> {
    let bytes = address.to_le_bytes();
    bytes
        .iter()
        .all(|b| (0x20..0x7F).contains(b))
        .then_some(bytes)
}

// ------------------------------------------------------------ the context

/// One enabled mod of a launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextMod {
    pub id: String,
    pub version: String,
    pub name: String,
}

/// What the toolkit writes for the module before a launch and whenever the
/// enabled mods change ([`CONTEXT_FILE`]): lines `key<TAB>value…`. The
/// module copies the file into a report as it is.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Context {
    /// Who wrote it: `doctor-toolkit 0.0.6 (mod manager)`.
    pub writer: String,
    /// When, `YYYY-MM-DD HH:MM:SS` UTC.
    pub written: String,
    /// How the mods reach the game: `written` into its folder, `staged`
    /// outside it (stage 46), or a workspace.
    pub source: String,
    /// The enabled mods in load order.
    pub mods: Vec<ContextMod>,
}

fn clean(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

impl Context {
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("writer\t{}\r\n", clean(&self.writer)));
        out.push_str(&format!("written\t{}\r\n", clean(&self.written)));
        out.push_str(&format!("source\t{}\r\n", clean(&self.source)));
        for m in &self.mods {
            out.push_str(&format!(
                "mod\t{}\t{}\t{}\r\n",
                clean(&m.id),
                clean(&m.version),
                clean(&m.name)
            ));
        }
        out
    }

    pub fn parse(text: &str) -> Self {
        let mut out = Self::default();
        for line in text.lines() {
            let mut parts = line.trim_end().split('\t');
            match parts.next() {
                Some("writer") => out.writer = parts.next().unwrap_or_default().to_owned(),
                Some("written") => out.written = parts.next().unwrap_or_default().to_owned(),
                Some("source") => out.source = parts.next().unwrap_or_default().to_owned(),
                Some("mod") => out.mods.push(ContextMod {
                    id: parts.next().unwrap_or_default().to_owned(),
                    version: parts.next().unwrap_or_default().to_owned(),
                    name: parts.next().unwrap_or_default().to_owned(),
                }),
                _ => {}
            }
        }
        out
    }
}

// ------------------------------------------------------------- the report

/// A report read back: its head lines and its sections (`[name]`, then
/// lines until the next section).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub head: Vec<String>,
    pub sections: Vec<(String, Vec<String>)>,
}

/// The short story of a report, for a list or a notice.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Summary {
    /// `2026-11-16 14:03:22 UTC`.
    pub time: String,
    /// `0xC0000005 ACCESS_VIOLATION (read of 0x00000014)`.
    pub exception: String,
    /// The crash address.
    pub address: Option<u32>,
    /// `godfather2.exe+0x0000C278`, or that it lies in no module.
    pub place: String,
    /// What the knowledge base says about the address itself, if anything.
    pub known: Vec<String>,
    /// The function around the address, by the knowledge base.
    pub function: String,
    /// The module that wrote the report and the build it saw.
    pub module: String,
}

impl Report {
    /// Reads a report; `None` when the text is not one.
    pub fn parse(text: &str) -> Option<Self> {
        let mut lines = text.lines();
        let first = lines.next()?;
        if !first.starts_with(REPORT_MAGIC) {
            return None;
        }
        let mut out = Self {
            head: vec![first.to_owned()],
            sections: Vec::new(),
        };
        for line in lines {
            let line = line.trim_end();
            if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                out.sections.push((name.to_owned(), Vec::new()));
            } else if let Some((_, body)) = out.sections.last_mut() {
                body.push(line.to_owned());
            } else {
                out.head.push(line.to_owned());
            }
        }
        for (_, body) in &mut out.sections {
            while body.last().is_some_and(String::is_empty) {
                body.pop();
            }
        }
        Some(out)
    }

    /// The lines of a section.
    pub fn section(&self, name: &str) -> &[String] {
        self.sections
            .iter()
            .find(|(n, _)| n == name)
            .map_or(&[], |(_, body)| body.as_slice())
    }

    /// The values of `key: value` lines of a section (or of the head, for
    /// an empty name).
    pub fn values<'a>(&'a self, section: &str, key: &'a str) -> impl Iterator<Item = &'a str> {
        let lines = if section.is_empty() {
            self.head.as_slice()
        } else {
            self.section(section)
        };
        lines.iter().filter_map(move |l| {
            l.strip_prefix(key)
                .and_then(|rest| rest.strip_prefix(':'))
                .map(str::trim)
        })
    }

    fn value(&self, section: &str, key: &str) -> String {
        self.values(section, key)
            .next()
            .unwrap_or_default()
            .to_owned()
    }

    pub fn summary(&self) -> Summary {
        let address_line = self.value("exception", "address");
        let (address, place) = match address_line.split_once(' ') {
            Some((a, rest)) => (a, rest.trim().to_owned()),
            None => (address_line.as_str(), String::new()),
        };
        Summary {
            time: self.value("", "time"),
            exception: self.value("exception", "code"),
            address: address
                .strip_prefix("0x")
                .and_then(|h| u32::from_str_radix(h, 16).ok()),
            place,
            known: self
                .values("exception", "known")
                .map(str::to_owned)
                .collect(),
            function: self.value("exception", "function"),
            module: self.head.first().cloned().unwrap_or_default(),
        }
    }
}

/// Whether a file name is a report's text (`crash-….txt`).
pub fn is_report_name(name: &str) -> bool {
    name.starts_with(REPORT_PREFIX) && name.ends_with(".txt")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_has_the_known_crashes_and_the_hooks() {
        let all: Vec<Symbol> = symbols(SYMBOLS).collect();
        assert!(all.len() > 900, "{}", all.len());
        assert!(all.windows(2).all(|w| w[0].address <= w[1].address));
        let known: Vec<&Symbol> = all.iter().filter(|s| s.address == 0x6E75_6F7B).collect();
        assert!(known.iter().any(|s| s.note == "known-crashes"), "{known:?}");
        assert!(all.iter().any(|s| s.address == 0x0040_8A00));
        assert!(all.iter().any(|s| s.address == LUA_THROW));
    }

    #[test]
    fn the_nearest_name_below_is_code_and_not_the_address_itself() {
        let table = "# c\n00408A00\tbus\tsend\n00408A05\tbus\tafter\n01162380\tbus\tdata\n";
        let s = symbol_before(table, 0x0040_8A05, 0).unwrap();
        assert_eq!((s.address, s.what), (0x0040_8A00, "send"));
        let s = symbol_before(table, 0x0040_8A40, 0).unwrap();
        assert_eq!(s.address, 0x0040_8A05);
        assert!(symbol_before(table, 0x0040_8A40, 0x0040_8A10).is_none());
        assert!(symbol_before(table, 0x0040_8A00, 0).is_none());
        // Data is never "the function before".
        assert_eq!(
            symbol_before(table, 0x0116_3000, 0).unwrap().address,
            0x0040_8A05
        );
    }

    #[test]
    fn a_function_starts_after_padding() {
        // 0x00401000: a function; 0x0040101E: its end and padding;
        // 0x00401020: the next one, with an aligned loop at 0x00401030.
        let mut image = vec![0x90u8; 0x100];
        image[0x1E] = 0xC3;
        image[0x1F] = 0xCC;
        let mut read = |va: u32, out: &mut [u8]| {
            let at = (va - TEXT_START) as usize;
            match image.get(at..at + out.len()) {
                Some(b) => {
                    out.copy_from_slice(b);
                    true
                }
                None => false,
            }
        };
        assert_eq!(function_start(&mut read, 0x0040_1034), Some(0x0040_1020));
        assert_eq!(function_start(&mut read, 0x0040_1020), Some(0x0040_1020));
        assert_eq!(function_start(&mut read, 0x0040_101D), Some(TEXT_START));
        assert_eq!(function_start(&mut read, 0x0030_0000), None);
    }

    #[test]
    fn the_known_crash_address_reads_as_text() {
        assert_eq!(as_text(0x6E75_6F7B).as_ref(), Some(b"{oun"));
        assert_eq!(as_text(0x0040_C278), None);
    }

    #[test]
    fn engine_messages_have_names() {
        let hash = crate::name_hash("iMsgRunningTick");
        assert_eq!(engine_message(hash), Some("iMsgRunningTick"));
        assert_eq!(engine_message(1), None);
    }

    #[test]
    fn the_hooks_lie_outside_what_the_other_modules_check() {
        let mine = [
            (BUS_HOOK, BUS_HOOK_BYTES.len() as u32),
            (LUA_THROW, LUA_THROW_BYTES.len() as u32),
        ];
        let theirs = crate::edition::CHECKS
            .iter()
            .chain(crate::render::CHECKS.iter());
        for check in theirs {
            let (start, end) = (check.address, check.address + check.bytes.len() as u32);
            for (at, len) in mine {
                assert!(
                    at + len <= start || at >= end,
                    "the hook at {at:#010x} overlaps the check of {:#010x} ({})",
                    check.address,
                    check.what
                );
            }
        }
        for (check, bytes) in [
            (&CHECKS[0], &BUS_HOOK_BYTES[..]),
            (&CHECKS[1], &LUA_THROW_BYTES[..]),
        ] {
            assert!(check.bytes.starts_with(bytes));
        }
    }

    /// The places the module checks and the start of functions found by
    /// padding, against the real executable (needs the game).
    #[test]
    fn crash_checks_match_the_game() {
        let Some(dir) = std::env::var_os("EARS_GF2_DIR") else {
            return;
        };
        let path = std::path::Path::new(&dir).join("godfather2.exe");
        let Ok(data) = std::fs::read(&path) else {
            return;
        };
        assert_eq!(crate::edition::check_file_with(&data, CHECKS), Ok(()));
        // `.text` lies at file offset 0x1000 for the address 0x00401000.
        let mut read = |va: u32, out: &mut [u8]| {
            let at = (va - crate::edition::IMAGE_BASE) as usize;
            match data.get(at..at + out.len()) {
                Some(b) => {
                    out.copy_from_slice(b);
                    true
                }
                None => false,
            }
        };
        // Known crashes inside known functions: the bus, the resource
        // registration (`-stream` with two streams), the screen fade.
        for (address, start) in [
            (0x0040_8A11, 0x0040_8A00),
            (LUA_THROW, LUA_THROW),
            (0x0042_138B, 0x0042_12B0),
            (0x0040_C278, 0x0040_C1F0),
            (0x0068_B04B, 0x0068_AEB0),
        ] {
            assert_eq!(
                function_start(&mut read, address),
                Some(start),
                "{address:#010x}"
            );
        }
        // Most of the named code addresses are starts of functions or lie
        // in a function whose start is found within 64 KB.
        let named: Vec<u32> = symbols(SYMBOLS)
            .map(|s| s.address)
            .filter(|&a| in_text(a))
            .collect();
        let found = named
            .iter()
            .filter(|&&a| function_start(&mut read, a).is_some())
            .count();
        assert_eq!(found, named.len());
        let starts = named
            .iter()
            .filter(|&&a| function_start(&mut read, a) == Some(a))
            .count();
        assert!(starts * 2 > named.len(), "{starts} of {}", named.len());
    }

    #[test]
    fn the_context_round_trips() {
        let c = Context {
            writer: "doctor-toolkit 0.0.6 (mod manager)".into(),
            written: "2026-11-16 12:00:00".into(),
            source: "written".into(),
            mods: vec![ContextMod {
                id: "new-york".into(),
                version: "1.1.0".into(),
                name: "New\tYork".into(),
            }],
        };
        let back = Context::parse(&c.to_text());
        assert_eq!(back.mods[0].name, "New York");
        assert_eq!(back.mods[0].version, "1.1.0");
        assert_eq!(back.writer, c.writer);
    }

    #[test]
    fn a_report_is_read_back() {
        let text = "doctor_crash 0.0.6 on PC 1.0.789.0 (build of 2009-10-05)\n\
time: 2026-11-16 14:03:22 UTC\n\n[exception]\n\
code: 0xC0000005 ACCESS_VIOLATION (read of 0x00000014)\n\
address: 0x0040C278 godfather2.exe+0x0000C278\n\
function: 0x0040C1F0 +0x88 fade\n\
known: [known-crashes] чтение по адресу 0x14\n\n[streams]\ngf2\\cuba.str\n";
        let report = Report::parse(text).unwrap();
        let s = report.summary();
        assert_eq!(s.time, "2026-11-16 14:03:22 UTC");
        assert_eq!(s.address, Some(0x0040_C278));
        assert_eq!(s.place, "godfather2.exe+0x0000C278");
        assert_eq!(s.known, ["[known-crashes] чтение по адресу 0x14"]);
        assert!(s.exception.starts_with("0xC0000005"));
        assert_eq!(report.section("streams"), ["gf2\\cuba.str"]);
        assert!(Report::parse("something else").is_none());
    }
}
