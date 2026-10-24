//! A minimal static ELF64 writer: one executable text segment, one writable data/bss segment.

use std::cell::Cell;

thread_local! {
    static LYTHOS: Cell<bool> = const { Cell::new(false) };
}

/// Whether the current compilation targets Lythos (instead of Linux).
pub fn lythos() -> bool {
    LYTHOS.with(|c| c.get())
}

/// Run `f` with the target set to Lythos (`true`) or Linux (`false`).
pub fn with_target<R>(lythos: bool, f: impl FnOnce() -> R) -> R {
    let old = LYTHOS.with(|c| c.replace(lythos));
    let r = f();
    LYTHOS.with(|c| c.set(old));
    r
}

/// Load address of the text segment. Lythos requires segments above 1 GiB.
pub fn text_base() -> u64 {
    if lythos() { 0x1_0000_0000 } else { 0x40_0000 }
}

/// Load address of the data segment.
pub fn data_base() -> u64 {
    text_base() + 0x40_0000
}

pub struct Image<'a> {
    pub text: &'a [u8],
    /// Entry point offset inside `text`.
    pub entry: u64,
    /// Initialized data placed at the data base.
    pub data: &'a [u8],
    /// Extra zeroed bytes after `data`.
    pub bss: u64,
    /// Non-loaded sections (debug information). Section headers are written only if any.
    pub extra: Vec<(&'static str, Vec<u8>)>,
}

const EHDR: u64 = 64;
const PHDR: u64 = 56;
const HDRS: u64 = EHDR + 2 * PHDR;

pub fn text_addr() -> u64 {
    text_base() + HDRS
}

pub fn write(img: &Image) -> Vec<u8> {
    let mut out = Vec::new();
    let text_off = HDRS;
    let text_va = text_base() + text_off;
    let data_off = (text_off + img.text.len() as u64 + 0xfff) & !0xfff;
    // Header.
    out.extend_from_slice(&[0x7f, b'E', b'L', b'F', 2, 1, 1, 0]);
    out.extend_from_slice(&[0; 8]);
    out.extend_from_slice(&2u16.to_le_bytes()); // ET_EXEC
    out.extend_from_slice(&62u16.to_le_bytes()); // x86-64
    out.extend_from_slice(&1u32.to_le_bytes());
    out.extend_from_slice(&(text_va + img.entry).to_le_bytes());
    out.extend_from_slice(&EHDR.to_le_bytes()); // phoff
    out.extend_from_slice(&0u64.to_le_bytes()); // shoff
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&(EHDR as u16).to_le_bytes());
    out.extend_from_slice(&(PHDR as u16).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    // PT_LOAD text (R+X), covering headers + code.
    let seg = |out: &mut Vec<u8>, flags: u32, off: u64, va: u64, filesz: u64, memsz: u64| {
        out.extend_from_slice(&1u32.to_le_bytes());
        out.extend_from_slice(&flags.to_le_bytes());
        out.extend_from_slice(&off.to_le_bytes());
        out.extend_from_slice(&va.to_le_bytes());
        out.extend_from_slice(&va.to_le_bytes());
        out.extend_from_slice(&filesz.to_le_bytes());
        out.extend_from_slice(&memsz.to_le_bytes());
        out.extend_from_slice(&0x1000u64.to_le_bytes());
    };
    let tsz = text_off + img.text.len() as u64;
    seg(&mut out, 5, 0, text_base(), tsz, tsz);
    seg(&mut out, 6, data_off, data_base(), img.data.len() as u64, img.data.len() as u64 + img.bss);
    out.extend_from_slice(img.text);
    out.resize(data_off as usize, 0);
    out.extend_from_slice(img.data);
    if img.extra.is_empty() {
        return out;
    }
    // Section headers: null, .text, the extra sections, .shstrtab.
    let mut names = vec![0u8];
    let name_off = |n: &str, names: &mut Vec<u8>| {
        let o = names.len() as u32;
        names.extend_from_slice(n.as_bytes());
        names.push(0);
        o
    };
    let text_name = name_off(".text", &mut names);
    let mut extra: Vec<(u32, u64, u64)> = vec![];
    for (n, bytes) in &img.extra {
        let o = out.len() as u64;
        out.extend_from_slice(bytes);
        extra.push((name_off(n, &mut names), o, bytes.len() as u64));
    }
    let strtab_name = name_off(".shstrtab", &mut names);
    let strtab_off = out.len() as u64;
    out.extend_from_slice(&names);
    while out.len() % 8 != 0 {
        out.push(0);
    }
    let shoff = out.len() as u64;
    let sh = |out: &mut Vec<u8>, name: u32, ty: u32, flags: u64, addr: u64, off: u64, size: u64, align: u64| {
        out.extend_from_slice(&name.to_le_bytes());
        out.extend_from_slice(&ty.to_le_bytes());
        out.extend_from_slice(&flags.to_le_bytes());
        out.extend_from_slice(&addr.to_le_bytes());
        out.extend_from_slice(&off.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&align.to_le_bytes());
        out.extend_from_slice(&0u64.to_le_bytes());
    };
    sh(&mut out, 0, 0, 0, 0, 0, 0, 0);
    sh(&mut out, text_name, 1, 6, text_va, text_off, img.text.len() as u64, 16);
    for (n, o, sz) in &extra {
        sh(&mut out, *n, 1, 0, 0, *o, *sz, 1);
    }
    sh(&mut out, strtab_name, 3, 0, 0, strtab_off, names.len() as u64, 1);
    let count = 3 + extra.len() as u16;
    out[40..48].copy_from_slice(&shoff.to_le_bytes()); // e_shoff
    out[58..60].copy_from_slice(&64u16.to_le_bytes()); // e_shentsize
    out[60..62].copy_from_slice(&count.to_le_bytes()); // e_shnum
    out[62..64].copy_from_slice(&(count - 1).to_le_bytes()); // e_shstrndx
    out
}
