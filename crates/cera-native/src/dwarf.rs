//! Minimal DWARF 4: a line table and one subprogram per user function, so that
//! debuggers (gdb, lldb), `addr2line` and `perf` can show Cera source positions.

pub struct SrcFile {
    pub name: String,
    /// Offset of the file's first byte in the span coordinates of the program.
    pub base: usize,
    pub text: String,
}

pub struct FuncInfo {
    pub name: String,
    pub addr: u64,
    pub len: u64,
    /// `(address, span start)` in ascending address order.
    pub locs: Vec<(u64, u32)>,
}

fn uleb(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            break;
        }
        out.push(b | 0x80);
    }
}

fn sleb(out: &mut Vec<u8>, mut v: i64) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        let done = (v == 0 && b & 0x40 == 0) || (v == -1 && b & 0x40 != 0);
        if done {
            out.push(b);
            break;
        }
        out.push(b | 0x80);
    }
}

fn cstr(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(s.as_bytes());
    out.push(0);
}

/// `(file index starting at 1, line starting at 1)` for a span offset.
fn position(files: &[SrcFile], offset: u32) -> Option<(usize, u32)> {
    let offset = offset as usize;
    let (i, f) = files.iter().enumerate().rev().find(|(_, f)| f.base <= offset && offset <= f.base + f.text.len())?;
    let local = (offset - f.base).min(f.text.len());
    let line = f.text.as_bytes()[..local].iter().filter(|&&b| b == b'\n').count() as u32 + 1;
    Some((i + 1, line))
}

fn debug_line(files: &[SrcFile], funcs: &[FuncInfo], text_end: u64) -> Vec<u8> {
    let mut header = vec![1u8, 1, 1, 0xfb, 14, 13]; // min inst, max ops, default_is_stmt, line_base -5, line_range, opcode_base
    header.extend_from_slice(&[0, 1, 1, 1, 1, 0, 0, 0, 1, 0, 0, 1]);
    header.push(0); // no include directories
    for f in files {
        cstr(&mut header, &f.name);
        header.extend_from_slice(&[0, 0, 0]); // directory, mtime, length
    }
    header.push(0);

    let mut prog: Vec<u8> = vec![];
    let (mut cur_file, mut cur_line, mut cur_addr) = (1usize, 1i64, None::<u64>);
    for f in funcs {
        for &(addr, loc) in &f.locs {
            let Some((file, line)) = position(files, loc) else { continue };
            if cur_addr == Some(addr) {
                continue;
            }
            if Some((file, line as i64)) == (cur_addr.is_some()).then_some((cur_file, cur_line)) {
                continue; // same line as the previous row
            }
            match cur_addr {
                None => {
                    prog.extend_from_slice(&[0, 9, 2]);
                    prog.extend_from_slice(&addr.to_le_bytes());
                }
                Some(prev) => {
                    prog.push(2); // DW_LNS_advance_pc
                    uleb(&mut prog, addr - prev);
                }
            }
            if file != cur_file {
                prog.push(4); // DW_LNS_set_file
                uleb(&mut prog, file as u64);
                cur_file = file;
            }
            if line as i64 != cur_line {
                prog.push(3); // DW_LNS_advance_line
                sleb(&mut prog, line as i64 - cur_line);
                cur_line = line as i64;
            }
            prog.push(1); // DW_LNS_copy
            cur_addr = Some(addr);
        }
    }
    if let Some(prev) = cur_addr {
        prog.push(2);
        uleb(&mut prog, text_end.saturating_sub(prev));
    } else {
        prog.extend_from_slice(&[0, 9, 2]);
        prog.extend_from_slice(&text_end.to_le_bytes());
    }
    prog.extend_from_slice(&[0, 1, 1]); // DW_LNE_end_sequence

    let mut body = vec![];
    body.extend_from_slice(&4u16.to_le_bytes());
    body.extend_from_slice(&(header.len() as u32).to_le_bytes());
    body.extend_from_slice(&header);
    body.extend_from_slice(&prog);
    let mut out = (body.len() as u32).to_le_bytes().to_vec();
    out.extend_from_slice(&body);
    out
}

fn debug_abbrev() -> Vec<u8> {
    let mut a = vec![];
    // 1: compile unit
    a.extend_from_slice(&[1, 0x11, 1]);
    for (at, form) in [(0x25u8, 0x08u8), (0x13, 0x05), (0x03, 0x08), (0x1b, 0x08), (0x11, 0x01), (0x12, 0x07), (0x10, 0x17)] {
        a.extend_from_slice(&[at, form]);
    }
    a.extend_from_slice(&[0, 0]);
    // 2: subprogram
    a.extend_from_slice(&[2, 0x2e, 0]);
    for (at, form) in [(0x03u8, 0x08u8), (0x11, 0x01), (0x12, 0x07), (0x3f, 0x19)] {
        a.extend_from_slice(&[at, form]);
    }
    a.extend_from_slice(&[0, 0]);
    a.push(0);
    a
}

fn debug_info(funcs: &[FuncInfo], files: &[SrcFile], text_va: u64, text_end: u64) -> Vec<u8> {
    let mut body = vec![];
    body.extend_from_slice(&4u16.to_le_bytes());
    body.extend_from_slice(&0u32.to_le_bytes()); // abbrev offset
    body.push(8); // address size
    uleb(&mut body, 1);
    cstr(&mut body, concat!("cera ", env!("CARGO_PKG_VERSION")));
    body.extend_from_slice(&0x0cu16.to_le_bytes()); // DW_LANG_C99: debuggers have no Cera
    cstr(&mut body, files.first().map_or("<cera>", |f| f.name.as_str()));
    cstr(&mut body, &std::env::current_dir().map(|d| d.display().to_string()).unwrap_or_default());
    body.extend_from_slice(&text_va.to_le_bytes());
    body.extend_from_slice(&(text_end - text_va).to_le_bytes());
    body.extend_from_slice(&0u32.to_le_bytes()); // stmt_list
    for f in funcs {
        uleb(&mut body, 2);
        cstr(&mut body, &f.name);
        body.extend_from_slice(&f.addr.to_le_bytes());
        body.extend_from_slice(&f.len.to_le_bytes());
    }
    body.push(0);
    let mut out = (body.len() as u32).to_le_bytes().to_vec();
    out.extend_from_slice(&body);
    out
}

/// The `.debug_*` sections for a program laid out at `text_va..text_end`.
pub fn sections(files: &[SrcFile], funcs: &[FuncInfo], text_va: u64, text_end: u64) -> Vec<(&'static str, Vec<u8>)> {
    vec![
        (".debug_abbrev", debug_abbrev()),
        (".debug_info", debug_info(funcs, files, text_va, text_end)),
        (".debug_line", debug_line(files, funcs, text_end)),
    ]
}
