//! A small x86-64 assembler for `asm("...")` blocks: Intel syntax, labels, and the integer and
//! system instructions kernels and runtimes need. No SSE or AVX; use `.byte` for anything else.
//!
//! One instruction per line (or separated by `;`); `#` starts a comment; `name:` defines a
//! label. Operands: registers (`rax`, `eax`, `ax`, `al`, `r8` to `r15` with `d`/`w`/`b`
//! suffixes), control registers (`cr0` ...), numbers (`10`, `-4`, `0x1f`, `0b101`, `'a'`),
//! memory `[base + index*scale + disp]` with an optional `byte|word|dword|qword ptr` prefix,
//! and `[rip + label]`. Jumps and calls to labels always use 32-bit displacements.

use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq)]
enum Op {
    Reg(Reg),
    Imm(i128),
    Mem(Mem),
    Label(String),
    Cr(u8),
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Reg {
    n: u8,
    size: u8,
    /// spl, bpl, sil, dil, r8b ...: byte registers that need a REX prefix.
    rex_byte: bool,
    /// ah, ch, dh, bh: cannot be combined with a REX prefix.
    high: bool,
}

#[derive(Clone, Debug, PartialEq, Default)]
struct Mem {
    base: Option<u8>,
    index: Option<(u8, u8)>,
    disp: i64,
    /// `[rip + ...]`: the label to point at (empty for a plain displacement).
    rip: Option<String>,
    size: Option<u8>,
}

fn parse_reg(s: &str) -> Option<Reg> {
    let s = s.to_ascii_lowercase();
    let r = |n: u8, size: u8| Some(Reg { n, size, rex_byte: false, high: false });
    const Q: [&str; 8] = ["rax", "rcx", "rdx", "rbx", "rsp", "rbp", "rsi", "rdi"];
    const D: [&str; 8] = ["eax", "ecx", "edx", "ebx", "esp", "ebp", "esi", "edi"];
    const W: [&str; 8] = ["ax", "cx", "dx", "bx", "sp", "bp", "si", "di"];
    const B: [&str; 8] = ["al", "cl", "dl", "bl", "spl", "bpl", "sil", "dil"];
    const H: [&str; 4] = ["ah", "ch", "dh", "bh"];
    if let Some(i) = Q.iter().position(|x| *x == s) {
        return r(i as u8, 8);
    }
    if let Some(i) = D.iter().position(|x| *x == s) {
        return r(i as u8, 4);
    }
    if let Some(i) = W.iter().position(|x| *x == s) {
        return r(i as u8, 2);
    }
    if let Some(i) = B.iter().position(|x| *x == s) {
        return Some(Reg { n: i as u8, size: 1, rex_byte: i >= 4, high: false });
    }
    if let Some(i) = H.iter().position(|x| *x == s) {
        return Some(Reg { n: i as u8 + 4, size: 1, rex_byte: false, high: true });
    }
    if let Some(rest) = s.strip_prefix('r') {
        let (digits, size) = if let Some(d) = rest.strip_suffix('d') {
            (d, 4)
        } else if let Some(d) = rest.strip_suffix('w') {
            (d, 2)
        } else if let Some(d) = rest.strip_suffix('b').or_else(|| rest.strip_suffix('l')) {
            (d, 1)
        } else {
            (rest, 8)
        };
        if let Ok(n) = digits.parse::<u8>() {
            if (8..=15).contains(&n) {
                return Some(Reg { n, size, rex_byte: size == 1, high: false });
            }
        }
    }
    None
}

fn parse_num(s: &str) -> Option<i128> {
    let t = s.trim().replace('_', "");
    let (neg, t) = match t.strip_prefix('-') {
        Some(r) => (true, r.trim().to_string()),
        None => (false, t.strip_prefix('+').unwrap_or(&t).to_string()),
    };
    let v = if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        i128::from_str_radix(h, 16).ok()?
    } else if let Some(b) = t.strip_prefix("0b") {
        i128::from_str_radix(b, 2).ok()?
    } else if t.len() == 3 && t.starts_with('\'') && t.ends_with('\'') {
        t.as_bytes()[1] as i128
    } else {
        t.parse::<i128>().ok()?
    };
    Some(if neg { -v } else { v })
}

fn is_ident(s: &str) -> bool {
    !s.is_empty()
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '$')
        && !s.chars().next().unwrap().is_ascii_digit()
}

fn parse_operand(s: &str) -> Result<Op, String> {
    let s = s.trim();
    let lower = s.to_ascii_lowercase();
    let mut size = None;
    let mut rest = s;
    for (kw, sz) in [("byte", 1u8), ("word", 2), ("dword", 4), ("qword", 8)] {
        if let Some(r) = lower.strip_prefix(kw) {
            let r = r.trim_start();
            if let Some(r2) = r.strip_prefix("ptr") {
                size = Some(sz);
                rest = s[s.len() - r2.len()..].trim();
                break;
            }
        }
    }
    if rest.starts_with('[') {
        let inner = rest
            .strip_prefix('[')
            .and_then(|r| r.strip_suffix(']'))
            .ok_or_else(|| format!("bad memory operand `{s}`"))?;
        let mut m = Mem { size, ..Default::default() };
        let mut terms: Vec<(bool, String)> = vec![];
        let mut cur = String::new();
        let mut sign = true;
        for c in inner.chars() {
            if c == '+' || c == '-' {
                if !cur.trim().is_empty() {
                    terms.push((sign, cur.trim().to_string()));
                    cur.clear();
                }
                sign = c == '+';
            } else {
                cur.push(c);
            }
        }
        if !cur.trim().is_empty() {
            terms.push((sign, cur.trim().to_string()));
        }
        for (pos, t) in terms {
            if let Some((a, b)) = t.split_once('*') {
                let (reg, sc) = match (parse_reg(a.trim()), parse_reg(b.trim())) {
                    (Some(r), None) => (r, parse_num(b).ok_or("bad scale")?),
                    (None, Some(r)) => (r, parse_num(a).ok_or("bad scale")?),
                    _ => return Err(format!("bad index term `{t}`")),
                };
                if !matches!(sc, 1 | 2 | 4 | 8) || !pos {
                    return Err(format!("bad scale in `{t}`"));
                }
                if reg.n == 4 {
                    return Err("rsp cannot be an index".into());
                }
                if m.index.is_some() {
                    return Err("two index registers".into());
                }
                m.index = Some((reg.n, sc as u8));
            } else if let Some(r) = parse_reg(&t) {
                if r.size != 8 {
                    return Err(format!("address registers must be 64-bit, found `{t}`"));
                }
                if !pos {
                    return Err("cannot subtract a register".into());
                }
                if m.base.is_none() {
                    m.base = Some(r.n);
                } else if m.index.is_none() {
                    if r.n == 4 {
                        return Err("rsp cannot be an index".into());
                    }
                    m.index = Some((r.n, 1));
                } else {
                    return Err("too many registers in address".into());
                }
            } else if t.eq_ignore_ascii_case("rip") {
                m.rip = Some(String::new());
            } else if let Some(n) = parse_num(&t) {
                m.disp += if pos { n as i64 } else { -(n as i64) };
            } else if is_ident(&t) {
                m.rip = Some(t);
            } else {
                return Err(format!("cannot understand `{t}` in an address"));
            }
        }
        return Ok(Op::Mem(m));
    }
    if let Some(r) = parse_reg(rest) {
        return Ok(Op::Reg(r));
    }
    if let Some(n) = lower.strip_prefix("cr") {
        if let Ok(i) = n.parse::<u8>() {
            return Ok(Op::Cr(i));
        }
    }
    if let Some(n) = parse_num(rest) {
        return Ok(Op::Imm(n));
    }
    if is_ident(rest) {
        return Ok(Op::Label(rest.to_string()));
    }
    Err(format!("cannot understand operand `{s}`"))
}

struct Fix {
    at: usize,
    label: String,
    /// The displacement is relative to this offset (the end of the instruction).
    end: usize,
}

struct Asm {
    out: Vec<u8>,
    labels: HashMap<String, usize>,
    fixes: Vec<Fix>,
    pending_rip: Option<(usize, String)>,
}

fn size_of(ops: &[Op]) -> Option<u8> {
    for o in ops {
        if let Op::Reg(r) = o {
            return Some(r.size);
        }
    }
    for o in ops {
        if let Op::Mem(m) = o {
            if let Some(s) = m.size {
                return Some(s);
            }
        }
    }
    None
}

fn fits8(v: i128) -> bool {
    (-128..=127).contains(&v)
}

/// Fold an unsigned literal into the signed range of an operand of `size` bytes.
fn norm(v: i128, size: u8) -> i128 {
    match size {
        1 if (128..=255).contains(&v) => v - 256,
        2 if (32768..=65535).contains(&v) => v - 65536,
        4 if (1i128 << 31..1i128 << 32).contains(&v) => v - (1i128 << 32),
        _ => v,
    }
}

fn imm_bytes(v: i128, n: usize) -> Result<Vec<u8>, String> {
    let ok = match n {
        1 => (-128..=255).contains(&v),
        2 => (-32768..=65535).contains(&v),
        4 => (-(1i128 << 31)..(1i128 << 32)).contains(&v),
        _ => (-(1i128 << 63)..(1i128 << 64)).contains(&v),
    };
    if !ok {
        return Err(format!("immediate {v} does not fit in {n} byte(s)"));
    }
    Ok((v as i64).to_le_bytes()[..n].to_vec())
}

const CCS: [(&str, u8); 30] = [
    ("o", 0), ("no", 1), ("b", 2), ("c", 2), ("nae", 2), ("ae", 3), ("nb", 3), ("nc", 3), ("e", 4), ("z", 4),
    ("ne", 5), ("nz", 5), ("be", 6), ("na", 6), ("a", 7), ("nbe", 7), ("s", 8), ("ns", 9), ("p", 10), ("pe", 10),
    ("np", 11), ("po", 11), ("l", 12), ("nge", 12), ("ge", 13), ("nl", 13), ("le", 14), ("ng", 14), ("g", 15), ("nle", 15),
];

fn cc(name: &str) -> Option<u8> {
    CCS.iter().find(|(n, _)| *n == name).map(|(_, v)| *v)
}

impl Asm {
    fn new() -> Asm {
        Asm { out: vec![], labels: HashMap::new(), fixes: vec![], pending_rip: None }
    }

    fn emit(&mut self, bytes: &[u8]) {
        self.out.extend_from_slice(bytes);
    }

    /// `[0x66] [mandatory] [REX] opcode modrm [sib] [disp] imm`. `reg` is the ModRM reg field
    /// (a register number or an opcode extension), `rm` the register or memory operand.
    fn enc(&mut self, size: u8, mandatory: Option<u8>, opcode: &[u8], reg: u8, rm: &Op, imm: &[u8], reg_op: Option<&Reg>) -> Result<(), String> {
        if size == 2 {
            self.out.push(0x66);
        }
        if let Some(p) = mandatory {
            self.out.push(p);
        }
        let mut rex = 0x40u8;
        if size == 8 {
            rex |= 8;
        }
        if reg >= 8 {
            rex |= 4;
        }
        let mut need_rex = false;
        let mut high_used = reg_op.is_some_and(|r| r.high);
        if reg_op.is_some_and(|r| r.rex_byte) {
            need_rex = true;
        }
        match rm {
            Op::Reg(r) => {
                if r.n >= 8 {
                    rex |= 1;
                }
                need_rex |= r.rex_byte;
                high_used |= r.high;
            }
            Op::Mem(m) => {
                if m.base.is_some_and(|b| b >= 8) {
                    rex |= 1;
                }
                if m.index.is_some_and(|(i, _)| i >= 8) {
                    rex |= 2;
                }
            }
            _ => return Err("bad operand".into()),
        }
        if rex != 0x40 || need_rex {
            if high_used {
                return Err("ah, ch, dh and bh cannot be used with this operand".into());
            }
            self.out.push(rex);
        }
        self.out.extend_from_slice(opcode);
        let r3 = reg & 7;
        match rm {
            Op::Reg(r) => self.out.push(0xC0 | (r3 << 3) | (r.n & 7)),
            Op::Mem(m) => self.modrm_mem(r3, m)?,
            _ => unreachable!(),
        }
        let fix = self.pending_rip.take();
        self.out.extend_from_slice(imm);
        if let Some((at, label)) = fix {
            self.fixes.push(Fix { at, label, end: self.out.len() });
        }
        Ok(())
    }

    fn modrm_mem(&mut self, r3: u8, m: &Mem) -> Result<(), String> {
        if let Some(label) = &m.rip {
            if m.base.is_some() || m.index.is_some() {
                return Err("[rip + ...] cannot have registers".into());
            }
            self.out.push((r3 << 3) | 5);
            let at = self.out.len();
            self.out.extend_from_slice(&(m.disp as i32).to_le_bytes());
            if !label.is_empty() {
                self.pending_rip = Some((at, label.clone()));
            }
            return Ok(());
        }
        let disp = m.disp;
        if i32::try_from(disp).is_err() {
            return Err("displacement does not fit in 32 bits".into());
        }
        let (md, dbytes): (u8, Vec<u8>) = match m.base {
            None => (0, (disp as i32).to_le_bytes().to_vec()),
            Some(b) => {
                if disp == 0 && (b & 7) != 5 {
                    (0, vec![])
                } else if fits8(disp as i128) {
                    (1, vec![disp as i8 as u8])
                } else {
                    (2, (disp as i32).to_le_bytes().to_vec())
                }
            }
        };
        let need_sib = m.index.is_some() || m.base.is_none() || m.base.is_some_and(|b| b & 7 == 4);
        if need_sib {
            self.out.push((md << 6) | (r3 << 3) | 4);
            let (idx, sc) = m.index.unwrap_or((4, 1));
            let ss = match sc {
                1 => 0,
                2 => 1,
                4 => 2,
                _ => 3,
            };
            let base = m.base.map_or(5, |b| b & 7);
            self.out.push((ss << 6) | ((idx & 7) << 3) | base);
        } else {
            self.out.push((md << 6) | (r3 << 3) | (m.base.unwrap() & 7));
        }
        self.out.extend_from_slice(&dbytes);
        Ok(())
    }

    /// An opcode with the register number added to its last byte (`push`, `pop`, `bswap`, `mov r, imm`).
    fn enc_plus(&mut self, size: u8, mandatory: Option<u8>, opcode: &[u8], r: &Reg, imm: &[u8], rex_w_for_8: bool) {
        if size == 2 {
            self.out.push(0x66);
        }
        if let Some(p) = mandatory {
            self.out.push(p);
        }
        let mut rex = 0x40u8;
        if size == 8 && rex_w_for_8 {
            rex |= 8;
        }
        if r.n >= 8 {
            rex |= 1;
        }
        if rex != 0x40 || r.rex_byte {
            self.out.push(rex);
        }
        let (last, head) = opcode.split_last().unwrap();
        self.out.extend_from_slice(head);
        self.out.push(last + (r.n & 7));
        self.out.extend_from_slice(imm);
    }

    fn label_rel32(&mut self, label: &str) {
        let at = self.out.len();
        self.out.extend_from_slice(&[0, 0, 0, 0]);
        self.fixes.push(Fix { at, label: label.to_string(), end: at + 4 });
    }

    fn ins(&mut self, mnem: &str, ops: &[Op]) -> Result<(), String> {
        let m = mnem;
        let size = size_of(ops);
        let need = |s: Option<u8>| s.ok_or_else(|| format!("`{m}`: say the operand size, e.g. `dword ptr [..]`"));
        match (m, ops) {
            // ---- no operands
            ("nop", []) => self.emit(&[0x90]),
            ("pause", []) => self.emit(&[0xF3, 0x90]),
            ("hlt", []) => self.emit(&[0xF4]),
            ("int3", []) => self.emit(&[0xCC]),
            ("syscall", []) => self.emit(&[0x0F, 0x05]),
            ("sysret", []) | ("sysretl", []) => self.emit(&[0x0F, 0x07]),
            ("sysretq", []) => self.emit(&[0x48, 0x0F, 0x07]),
            ("sysenter", []) => self.emit(&[0x0F, 0x34]),
            ("sysexit", []) => self.emit(&[0x0F, 0x35]),
            ("cpuid", []) => self.emit(&[0x0F, 0xA2]),
            ("rdtsc", []) => self.emit(&[0x0F, 0x31]),
            ("rdtscp", []) => self.emit(&[0x0F, 0x01, 0xF9]),
            ("rdpmc", []) => self.emit(&[0x0F, 0x33]),
            ("rdmsr", []) => self.emit(&[0x0F, 0x32]),
            ("wrmsr", []) => self.emit(&[0x0F, 0x30]),
            ("xgetbv", []) => self.emit(&[0x0F, 0x01, 0xD0]),
            ("cli", []) => self.emit(&[0xFA]),
            ("sti", []) => self.emit(&[0xFB]),
            ("clc", []) => self.emit(&[0xF8]),
            ("stc", []) => self.emit(&[0xF9]),
            ("cmc", []) => self.emit(&[0xF5]),
            ("cld", []) => self.emit(&[0xFC]),
            ("std", []) => self.emit(&[0xFD]),
            ("lfence", []) => self.emit(&[0x0F, 0xAE, 0xE8]),
            ("mfence", []) => self.emit(&[0x0F, 0xAE, 0xF0]),
            ("sfence", []) => self.emit(&[0x0F, 0xAE, 0xF8]),
            ("ud2", []) => self.emit(&[0x0F, 0x0B]),
            ("pushfq", []) | ("pushf", []) => self.emit(&[0x9C]),
            ("popfq", []) | ("popf", []) => self.emit(&[0x9D]),
            ("iretq", []) => self.emit(&[0x48, 0xCF]),
            ("iret", []) | ("iretd", []) => self.emit(&[0xCF]),
            ("swapgs", []) => self.emit(&[0x0F, 0x01, 0xF8]),
            ("wbinvd", []) => self.emit(&[0x0F, 0x09]),
            ("invd", []) => self.emit(&[0x0F, 0x08]),
            ("leave", []) => self.emit(&[0xC9]),
            ("cdq", []) => self.emit(&[0x99]),
            ("cqo", []) => self.emit(&[0x48, 0x99]),
            ("cwd", []) => self.emit(&[0x66, 0x99]),
            ("cdqe", []) => self.emit(&[0x48, 0x98]),
            ("cwde", []) => self.emit(&[0x98]),
            ("cbw", []) => self.emit(&[0x66, 0x98]),
            ("sahf", []) => self.emit(&[0x9E]),
            ("lahf", []) => self.emit(&[0x9F]),
            ("movsb", []) => self.emit(&[0xA4]),
            ("movsw", []) => self.emit(&[0x66, 0xA5]),
            ("movsd", []) => self.emit(&[0xA5]),
            ("movsq", []) => self.emit(&[0x48, 0xA5]),
            ("cmpsb", []) => self.emit(&[0xA6]),
            ("cmpsw", []) => self.emit(&[0x66, 0xA7]),
            ("cmpsd", []) => self.emit(&[0xA7]),
            ("cmpsq", []) => self.emit(&[0x48, 0xA7]),
            ("stosb", []) => self.emit(&[0xAA]),
            ("stosw", []) => self.emit(&[0x66, 0xAB]),
            ("stosd", []) => self.emit(&[0xAB]),
            ("stosq", []) => self.emit(&[0x48, 0xAB]),
            ("lodsb", []) => self.emit(&[0xAC]),
            ("lodsw", []) => self.emit(&[0x66, 0xAD]),
            ("lodsd", []) => self.emit(&[0xAD]),
            ("lodsq", []) => self.emit(&[0x48, 0xAD]),
            ("scasb", []) => self.emit(&[0xAE]),
            ("scasw", []) => self.emit(&[0x66, 0xAF]),
            ("scasd", []) => self.emit(&[0xAF]),
            ("scasq", []) => self.emit(&[0x48, 0xAF]),
            ("int", [Op::Imm(n)]) => {
                self.emit(&[0xCD]);
                self.emit(&imm_bytes(*n, 1)?);
            }
            // `ret` leaves the whole block (see `wrap`), so it becomes a jump to the epilogue.
            ("ret", []) => {
                self.emit(&[0xE9]);
                self.label_rel32("__asm_end");
            }

            // ---- in / out
            ("in", [Op::Reg(d), Op::Reg(p)]) if p.n == 2 && p.size == 2 && d.n == 0 => match d.size {
                1 => self.emit(&[0xEC]),
                2 => self.emit(&[0x66, 0xED]),
                _ => self.emit(&[0xED]),
            },
            ("in", [Op::Reg(d), Op::Imm(p)]) if d.n == 0 => {
                match d.size {
                    1 => self.emit(&[0xE4]),
                    2 => self.emit(&[0x66, 0xE5]),
                    _ => self.emit(&[0xE5]),
                }
                self.emit(&imm_bytes(*p, 1)?);
            }
            ("out", [Op::Reg(p), Op::Reg(s)]) if p.n == 2 && p.size == 2 && s.n == 0 => match s.size {
                1 => self.emit(&[0xEE]),
                2 => self.emit(&[0x66, 0xEF]),
                _ => self.emit(&[0xEF]),
            },
            ("out", [Op::Imm(p), Op::Reg(s)]) if s.n == 0 => {
                match s.size {
                    1 => self.emit(&[0xE6]),
                    2 => self.emit(&[0x66, 0xE7]),
                    _ => self.emit(&[0xE7]),
                }
                self.emit(&imm_bytes(*p, 1)?);
            }

            // ---- ALU group
            (_, [dst, src]) if alu_index(m).is_some() => {
                let idx = alu_index(m).unwrap();
                match (dst, src) {
                    (Op::Reg(_) | Op::Mem(_), Op::Reg(s)) => {
                        let op = idx * 8 + if s.size == 1 { 0 } else { 1 };
                        self.enc(s.size, None, &[op], s.n, dst, &[], Some(s))?;
                    }
                    (Op::Reg(d), Op::Mem(_)) => {
                        let op = idx * 8 + if d.size == 1 { 2 } else { 3 };
                        self.enc(d.size, None, &[op], d.n, src, &[], Some(d))?;
                    }
                    (Op::Reg(_) | Op::Mem(_), Op::Imm(v)) => {
                        let sz = need(size)?;
                        let v = norm(*v, sz);
                        if let Op::Reg(d) = dst {
                            if d.n == 0 && !d.high {
                                if sz == 1 {
                                    self.emit(&[idx * 8 + 4]);
                                    self.emit(&imm_bytes(v, 1)?);
                                    return Ok(());
                                } else if !fits8(v) {
                                    if sz == 2 {
                                        self.emit(&[0x66]);
                                    } else if sz == 8 {
                                        self.emit(&[0x48]);
                                    }
                                    self.emit(&[idx * 8 + 5]);
                                    self.emit(&imm_bytes(v, if sz == 2 { 2 } else { 4 })?);
                                    return Ok(());
                                }
                            }
                        }
                        let dreg = if let Op::Reg(d) = dst { Some(*d) } else { None };
                        if sz == 1 {
                            self.enc(1, None, &[0x80], idx, dst, &imm_bytes(v, 1)?, dreg.as_ref())?;
                        } else if fits8(v) {
                            self.enc(sz, None, &[0x83], idx, dst, &imm_bytes(v, 1)?, None)?;
                        } else {
                            if sz == 8 && !(-(1i128 << 31)..(1i128 << 31)).contains(&v) {
                                return Err(format!("`{m}`: immediate {v} does not fit in 32 bits"));
                            }
                            self.enc(sz, None, &[0x81], idx, dst, &imm_bytes(v, if sz == 2 { 2 } else { 4 })?, None)?;
                        }
                    }
                    _ => return Err(format!("`{m}`: unsupported operands")),
                }
            }

            // ---- mov
            ("mov", [Op::Reg(d), Op::Imm(v)]) => {
                match d.size {
                    1 => self.enc_plus(1, None, &[0xB0], d, &imm_bytes(*v, 1)?, false),
                    2 => self.enc_plus(2, None, &[0xB8], d, &imm_bytes(*v, 2)?, false),
                    4 => self.enc_plus(4, None, &[0xB8], d, &imm_bytes(*v, 4)?, false),
                    _ => {
                        if (-(1i128 << 31)..(1i128 << 31)).contains(v) {
                            self.enc(8, None, &[0xC7], 0, &Op::Reg(*d), &imm_bytes(*v, 4)?, None)?;
                        } else {
                            self.enc_plus(8, None, &[0xB8], d, &imm_bytes(*v, 8)?, true);
                        }
                    }
                }
            }
            ("mov", [Op::Mem(_), Op::Imm(v)]) => {
                let sz = need(size)?;
                let v = norm(*v, sz);
                if sz == 1 {
                    self.enc(1, None, &[0xC6], 0, &ops[0], &imm_bytes(v, 1)?, None)?;
                } else {
                    if sz == 8 && !(-(1i128 << 31)..(1i128 << 31)).contains(&v) {
                        return Err("mov: a memory store takes at most a 32-bit immediate".into());
                    }
                    self.enc(sz, None, &[0xC7], 0, &ops[0], &imm_bytes(v, if sz == 2 { 2 } else { 4 })?, None)?;
                }
            }
            ("mov", [Op::Reg(_) | Op::Mem(_), Op::Reg(s)]) => {
                self.enc(s.size, None, &[if s.size == 1 { 0x88 } else { 0x89 }], s.n, &ops[0], &[], Some(s))?;
            }
            ("mov", [Op::Reg(d), Op::Mem(_)]) => {
                self.enc(d.size, None, &[if d.size == 1 { 0x8A } else { 0x8B }], d.n, &ops[1], &[], Some(d))?;
            }
            ("mov", [Op::Cr(c), Op::Reg(r)]) if r.size == 8 => self.enc(4, None, &[0x0F, 0x22], *c, &Op::Reg(*r), &[], None)?,
            ("mov", [Op::Reg(r), Op::Cr(c)]) if r.size == 8 => self.enc(4, None, &[0x0F, 0x20], *c, &Op::Reg(*r), &[], None)?,

            // ---- movzx / movsx / movsxd
            ("movzx", [Op::Reg(d), src]) | ("movsx", [Op::Reg(d), src]) | ("movsxd", [Op::Reg(d), src]) => {
                let ss = match src {
                    Op::Reg(r) => r.size,
                    Op::Mem(mm) => mm.size.ok_or("movzx/movsx: say the source size, e.g. `byte ptr [..]`")?,
                    _ => return Err(format!("`{m}`: bad source")),
                };
                let sx = m != "movzx";
                match (ss, sx) {
                    (1, false) => self.enc(d.size, None, &[0x0F, 0xB6], d.n, src, &[], Some(d))?,
                    (2, false) => self.enc(d.size, None, &[0x0F, 0xB7], d.n, src, &[], Some(d))?,
                    (1, true) => self.enc(d.size, None, &[0x0F, 0xBE], d.n, src, &[], Some(d))?,
                    (2, true) => self.enc(d.size, None, &[0x0F, 0xBF], d.n, src, &[], Some(d))?,
                    (4, true) if d.size == 8 => self.enc(8, None, &[0x63], d.n, src, &[], None)?,
                    _ => return Err(format!("`{m}`: unsupported operand sizes")),
                }
            }
            ("lea", [Op::Reg(d), Op::Mem(_)]) => self.enc(d.size, None, &[0x8D], d.n, &ops[1], &[], None)?,

            // ---- test
            ("test", [Op::Reg(_) | Op::Mem(_), Op::Reg(s)]) => {
                self.enc(s.size, None, &[if s.size == 1 { 0x84 } else { 0x85 }], s.n, &ops[0], &[], Some(s))?;
            }
            ("test", [dst @ (Op::Reg(_) | Op::Mem(_)), Op::Imm(v)]) => {
                let sz = need(size)?;
                let v = norm(*v, sz);
                if let Op::Reg(d) = dst {
                    if d.n == 0 && !d.high {
                        if sz == 1 {
                            self.emit(&[0xA8]);
                            self.emit(&imm_bytes(v, 1)?);
                            return Ok(());
                        }
                        if sz == 2 {
                            self.emit(&[0x66]);
                        } else if sz == 8 {
                            self.emit(&[0x48]);
                        }
                        self.emit(&[0xA9]);
                        self.emit(&imm_bytes(v, if sz == 2 { 2 } else { 4 })?);
                        return Ok(());
                    }
                }
                let dreg = if let Op::Reg(d) = dst { Some(*d) } else { None };
                if sz == 1 {
                    self.enc(1, None, &[0xF6], 0, dst, &imm_bytes(v, 1)?, dreg.as_ref())?;
                } else {
                    self.enc(sz, None, &[0xF7], 0, dst, &imm_bytes(v, if sz == 2 { 2 } else { 4 })?, None)?;
                }
            }

            // ---- unary group
            ("inc" | "dec" | "neg" | "not" | "mul" | "div" | "idiv", [rm @ (Op::Reg(_) | Op::Mem(_))]) => {
                let sz = need(size)?;
                let (op8, op, ext) = match m {
                    "inc" => (0xFE, 0xFF, 0),
                    "dec" => (0xFE, 0xFF, 1),
                    "not" => (0xF6, 0xF7, 2),
                    "neg" => (0xF6, 0xF7, 3),
                    "mul" => (0xF6, 0xF7, 4),
                    "div" => (0xF6, 0xF7, 6),
                    _ => (0xF6, 0xF7, 7),
                };
                let r = if let Op::Reg(r) = rm { Some(*r) } else { None };
                self.enc(sz, None, &[if sz == 1 { op8 } else { op }], ext, rm, &[], r.as_ref())?;
            }
            ("imul", [rm @ (Op::Reg(_) | Op::Mem(_))]) => {
                let sz = need(size)?;
                let r = if let Op::Reg(r) = rm { Some(*r) } else { None };
                self.enc(sz, None, &[if sz == 1 { 0xF6 } else { 0xF7 }], 5, rm, &[], r.as_ref())?;
            }
            ("imul", [Op::Reg(d), rm @ (Op::Reg(_) | Op::Mem(_))]) => self.enc(d.size, None, &[0x0F, 0xAF], d.n, rm, &[], None)?,
            ("imul", [Op::Reg(d), rm @ (Op::Reg(_) | Op::Mem(_)), Op::Imm(v)]) => {
                let v = norm(*v, d.size);
                if fits8(v) {
                    self.enc(d.size, None, &[0x6B], d.n, rm, &imm_bytes(v, 1)?, None)?;
                } else {
                    self.enc(d.size, None, &[0x69], d.n, rm, &imm_bytes(v, if d.size == 2 { 2 } else { 4 })?, None)?;
                }
            }
            ("imul", [Op::Reg(d), Op::Imm(v)]) => {
                let rm = Op::Reg(*d);
                let v = norm(*v, d.size);
                if fits8(v) {
                    self.enc(d.size, None, &[0x6B], d.n, &rm, &imm_bytes(v, 1)?, None)?;
                } else {
                    self.enc(d.size, None, &[0x69], d.n, &rm, &imm_bytes(v, if d.size == 2 { 2 } else { 4 })?, None)?;
                }
            }

            // ---- shifts and rotates
            ("rol" | "ror" | "rcl" | "rcr" | "shl" | "sal" | "shr" | "sar", [rm @ (Op::Reg(_) | Op::Mem(_)), count]) => {
                let sz = need(size)?;
                let ext = match m {
                    "rol" => 0,
                    "ror" => 1,
                    "rcl" => 2,
                    "rcr" => 3,
                    "shl" | "sal" => 4,
                    "shr" => 5,
                    _ => 7,
                };
                let r = if let Op::Reg(r) = rm { Some(*r) } else { None };
                match count {
                    Op::Imm(1) => self.enc(sz, None, &[if sz == 1 { 0xD0 } else { 0xD1 }], ext, rm, &[], r.as_ref())?,
                    Op::Imm(n) => self.enc(sz, None, &[if sz == 1 { 0xC0 } else { 0xC1 }], ext, rm, &imm_bytes(*n, 1)?, r.as_ref())?,
                    Op::Reg(c) if c.n == 1 && c.size == 1 => {
                        self.enc(sz, None, &[if sz == 1 { 0xD2 } else { 0xD3 }], ext, rm, &[], r.as_ref())?
                    }
                    _ => return Err(format!("`{m}`: the count is a number or `cl`")),
                }
            }

            // ---- stack
            ("push", [Op::Reg(r)]) if r.size == 8 => self.enc_plus(4, None, &[0x50], r, &[], false),
            ("pop", [Op::Reg(r)]) if r.size == 8 => self.enc_plus(4, None, &[0x58], r, &[], false),
            ("push", [Op::Imm(v)]) => {
                if fits8(*v) {
                    self.emit(&[0x6A]);
                    self.emit(&imm_bytes(*v, 1)?);
                } else {
                    self.emit(&[0x68]);
                    self.emit(&imm_bytes(*v, 4)?);
                }
            }
            ("push", [rm @ Op::Mem(_)]) => self.enc(4, None, &[0xFF], 6, rm, &[], None)?,
            ("pop", [rm @ Op::Mem(_)]) => self.enc(4, None, &[0x8F], 0, rm, &[], None)?,

            // ---- control flow
            ("jmp", [Op::Label(l)]) => {
                self.emit(&[0xE9]);
                self.label_rel32(l);
            }
            ("call", [Op::Label(l)]) => {
                self.emit(&[0xE8]);
                self.label_rel32(l);
            }
            ("jmp", [rm @ (Op::Reg(_) | Op::Mem(_))]) => self.enc(4, None, &[0xFF], 4, rm, &[], None)?,
            ("call", [rm @ (Op::Reg(_) | Op::Mem(_))]) => self.enc(4, None, &[0xFF], 2, rm, &[], None)?,
            ("ret", [Op::Imm(_)]) => return Err("`ret imm` is not supported inside asm blocks".into()),
            (_, [Op::Label(l)]) if m.starts_with('j') && cc(&m[1..]).is_some() => {
                self.emit(&[0x0F, 0x80 + cc(&m[1..]).unwrap()]);
                self.label_rel32(l);
            }
            (_, [rm @ (Op::Reg(_) | Op::Mem(_))]) if m.starts_with("set") && cc(&m[3..]).is_some() => {
                let r = if let Op::Reg(r) = rm { Some(*r) } else { None };
                self.enc(1, None, &[0x0F, 0x90 + cc(&m[3..]).unwrap()], 0, rm, &[], r.as_ref())?;
            }
            (_, [Op::Reg(d), rm @ (Op::Reg(_) | Op::Mem(_))]) if m.starts_with("cmov") && cc(&m[4..]).is_some() => {
                self.enc(d.size, None, &[0x0F, 0x40 + cc(&m[4..]).unwrap()], d.n, rm, &[], None)?;
            }

            // ---- exchange and atomics
            ("xchg", [Op::Reg(a), Op::Reg(b)]) if a.size >= 2 && a.n == 0 && b.size == a.size && a.size != 2 && !(b.n == 0) => {
                self.enc_plus(a.size, None, &[0x90], b, &[], true)
            }
            ("xchg", [Op::Reg(a), Op::Reg(b)]) if a.size >= 2 && b.n == 0 && b.size == a.size && a.size != 2 && a.n != 0 => {
                self.enc_plus(a.size, None, &[0x90], a, &[], true)
            }
            ("xchg", [Op::Reg(_) | Op::Mem(_), Op::Reg(s)]) => {
                self.enc(s.size, None, &[if s.size == 1 { 0x86 } else { 0x87 }], s.n, &ops[0], &[], Some(s))?
            }
            ("xchg", [Op::Reg(d), rm @ Op::Mem(_)]) => self.enc(d.size, None, &[if d.size == 1 { 0x86 } else { 0x87 }], d.n, rm, &[], Some(d))?,
            ("xadd", [rm @ (Op::Reg(_) | Op::Mem(_)), Op::Reg(s)]) => {
                self.enc(s.size, None, &[0x0F, if s.size == 1 { 0xC0 } else { 0xC1 }], s.n, rm, &[], Some(s))?
            }
            ("cmpxchg", [rm @ (Op::Reg(_) | Op::Mem(_)), Op::Reg(s)]) => {
                self.enc(s.size, None, &[0x0F, if s.size == 1 { 0xB0 } else { 0xB1 }], s.n, rm, &[], Some(s))?
            }
            ("bswap", [Op::Reg(r)]) if r.size >= 4 => self.enc_plus(r.size, None, &[0x0F, 0xC8], r, &[], true),

            // ---- bit tests and counting
            ("bt" | "bts" | "btr" | "btc", [rm @ (Op::Reg(_) | Op::Mem(_)), Op::Reg(s)]) => {
                let op = match m {
                    "bt" => 0xA3,
                    "bts" => 0xAB,
                    "btr" => 0xB3,
                    _ => 0xBB,
                };
                self.enc(s.size, None, &[0x0F, op], s.n, rm, &[], None)?;
            }
            ("bt" | "bts" | "btr" | "btc", [rm @ (Op::Reg(_) | Op::Mem(_)), Op::Imm(n)]) => {
                let sz = need(size)?;
                let ext = match m {
                    "bt" => 4,
                    "bts" => 5,
                    "btr" => 6,
                    _ => 7,
                };
                self.enc(sz, None, &[0x0F, 0xBA], ext, rm, &imm_bytes(*n, 1)?, None)?;
            }
            ("bsf" | "bsr", [Op::Reg(d), rm @ (Op::Reg(_) | Op::Mem(_))]) => {
                self.enc(d.size, None, &[0x0F, if m == "bsf" { 0xBC } else { 0xBD }], d.n, rm, &[], None)?
            }
            ("popcnt" | "lzcnt" | "tzcnt", [Op::Reg(d), rm @ (Op::Reg(_) | Op::Mem(_))]) => {
                let op = match m {
                    "popcnt" => 0xB8,
                    "lzcnt" => 0xBD,
                    _ => 0xBC,
                };
                self.enc(d.size, Some(0xF3), &[0x0F, op], d.n, rm, &[], None)?;
            }

            // ---- system
            ("invlpg" | "lgdt" | "lidt" | "sgdt" | "sidt" | "clflush", [rm @ Op::Mem(_)]) => {
                let (op, ext): (&[u8], u8) = match m {
                    "invlpg" => (&[0x0F, 0x01], 7),
                    "lgdt" => (&[0x0F, 0x01], 2),
                    "lidt" => (&[0x0F, 0x01], 3),
                    "sgdt" => (&[0x0F, 0x01], 0),
                    "sidt" => (&[0x0F, 0x01], 1),
                    _ => (&[0x0F, 0xAE], 7),
                };
                self.enc(4, None, op, ext, rm, &[], None)?;
            }
            ("ltr" | "lldt" | "str", [rm @ (Op::Reg(_) | Op::Mem(_))]) => {
                let ext = match m {
                    "ltr" => 3,
                    "lldt" => 2,
                    _ => 1,
                };
                self.enc(4, None, &[0x0F, 0x00], ext, rm, &[], None)?;
            }
            _ => return Err(format!("unsupported instruction `{m}` with {} operand(s)", ops.len())),
        }
        Ok(())
    }
}

fn alu_index(m: &str) -> Option<u8> {
    Some(match m {
        "add" => 0,
        "or" => 1,
        "adc" => 2,
        "sbb" => 3,
        "and" => 4,
        "sub" => 5,
        "xor" => 6,
        "cmp" => 7,
        _ => return None,
    })
}

fn split_operands(s: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut depth = 0;
    for c in s.chars() {
        match c {
            '[' => {
                depth += 1;
                cur.push(c);
            }
            ']' => {
                depth -= 1;
                cur.push(c);
            }
            ',' if depth == 0 => {
                out.push(cur.trim().to_string());
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

fn assemble_body(src: &str) -> Result<Asm, String> {
    let mut a = Asm::new();
    for (ln, raw) in src.lines().enumerate() {
        let no_comment = raw.split('#').next().unwrap_or("");
        for part in no_comment.split(';') {
            let mut line = part.trim();
            if line.is_empty() {
                continue;
            }
            let err = |e: String| format!("asm line {}: {e}", ln + 1);
            // Labels (several may precede an instruction).
            while let Some((head, rest)) = line.split_once(':') {
                let head = head.trim();
                if is_ident(head) && parse_reg(head).is_none() && !head.contains(' ') {
                    if a.labels.insert(head.to_string(), a.out.len()).is_some() {
                        return Err(err(format!("label `{head}` is defined twice")));
                    }
                    line = rest.trim();
                } else {
                    break;
                }
            }
            if line.is_empty() {
                continue;
            }
            let (word, rest) = match line.split_once(char::is_whitespace) {
                Some((w, r)) => (w.to_ascii_lowercase(), r.trim()),
                None => (line.to_ascii_lowercase(), ""),
            };
            // Prefixes.
            let mut word = word;
            let mut rest = rest.to_string();
            loop {
                let p: Option<u8> = match word.as_str() {
                    "lock" => Some(0xF0),
                    "rep" | "repe" | "repz" => Some(0xF3),
                    "repne" | "repnz" => Some(0xF2),
                    _ => None,
                };
                let Some(p) = p else { break };
                a.out.push(p);
                let (w, r) = match rest.split_once(char::is_whitespace) {
                    Some((w, r)) => (w.to_ascii_lowercase(), r.trim().to_string()),
                    None => (rest.to_ascii_lowercase(), String::new()),
                };
                if w.is_empty() {
                    return Err(err("a prefix needs an instruction".into()));
                }
                word = w;
                rest = r;
            }
            // Data directives.
            let dsize = match word.as_str() {
                ".byte" | "db" => Some(1),
                ".word" | "dw" => Some(2),
                ".long" | ".dword" | "dd" => Some(4),
                ".quad" | ".qword" | "dq" => Some(8),
                _ => None,
            };
            if let Some(n) = dsize {
                for tok in split_operands(&rest) {
                    let v = parse_num(&tok).ok_or_else(|| err(format!("`{tok}` is not a number")))?;
                    let b = imm_bytes(v, n).map_err(&err)?;
                    a.out.extend_from_slice(&b);
                }
                continue;
            }
            if word == ".align" {
                let n = parse_num(&rest).filter(|n| *n > 0 && (*n as usize).is_power_of_two()).ok_or_else(|| err(".align takes a power of two".into()))? as usize;
                while a.out.len() % n != 0 {
                    a.out.push(0x90);
                }
                continue;
            }
            let ops: Vec<Op> = split_operands(&rest).iter().map(|s| parse_operand(s)).collect::<Result<_, _>>().map_err(&err)?;
            a.ins(&word, &ops).map_err(&err)?;
        }
    }
    Ok(a)
}

fn resolve(a: &mut Asm) -> Result<(), String> {
    for f in std::mem::take(&mut a.fixes) {
        let target = *a.labels.get(&f.label).ok_or_else(|| format!("asm: unknown label `{}`", f.label))?;
        let rel = target as i64 - f.end as i64;
        let rel = i32::try_from(rel).map_err(|_| "asm: jump too far".to_string())?;
        a.out[f.at..f.at + 4].copy_from_slice(&rel.to_le_bytes());
    }
    Ok(())
}

/// Assemble a block. The result is a complete SysV function: arguments arrive in `rdi, rsi, rdx,
/// rcx, r8, r9`, the result is `rax`, `rbx rbp r12 to r15` are preserved, and `ret` anywhere in
/// the text leaves the block.
pub fn assemble(src: &str) -> Result<Vec<u8>, String> {
    let mut a = assemble_body(src)?;
    // Prologue: push rbx, rbp, r12-r15; sub rsp, 8 (keeps the stack 16-byte aligned).
    let pro: Vec<u8> = vec![0x53, 0x55, 0x41, 0x54, 0x41, 0x55, 0x41, 0x56, 0x41, 0x57, 0x48, 0x83, 0xEC, 0x08];
    let epi: Vec<u8> = vec![0x48, 0x83, 0xC4, 0x08, 0x41, 0x5F, 0x41, 0x5E, 0x41, 0x5D, 0x41, 0x5C, 0x5D, 0x5B, 0xC3];
    // The epilogue label sits after the body; every offset shifts by the prologue length.
    a.labels.insert("__asm_end".to_string(), a.out.len());
    resolve(&mut a)?;
    let mut out = pro;
    out.extend(a.out);
    out.extend(epi);
    Ok(out)
}

/// Assemble without the wrapper (for tests that compare encodings).
pub fn assemble_raw(src: &str) -> Result<Vec<u8>, String> {
    let mut a = assemble_body(src)?;
    resolve(&mut a)?;
    Ok(a.out)
}
