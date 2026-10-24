//! Cranelift code generation plus our own linking into a static ELF.

use crate::elf;
use cranelift_codegen::control::ControlPlane;
use cranelift_codegen::ir::{self, ExternalName, Function, UserExternalName};
use cranelift_codegen::isa::{self, CallConv, TargetIsa};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_codegen::{Context, FinalizedRelocTarget};
use std::str::FromStr;
use std::sync::Arc;

/// Function index used for the raw syscall stub.
pub const SYSCALL: u32 = u32::MAX;

pub struct Backend {
    pub isa: Arc<dyn TargetIsa>,
    code: Vec<Option<Vec<u8>>>,
    relocs: Vec<Vec<(u32, u32, i64, bool)>>, // (offset, target, addend, absolute)
    pub data: Vec<u8>,
    /// Source files for debug information (None: no DWARF).
    pub debug: Option<Vec<crate::dwarf::SrcFile>>,
    /// User-visible function names by id, for debug information.
    pub names: std::collections::HashMap<u32, String>,
    /// Per function: `(code offset, span start)` rows from code generation.
    locs: Vec<Vec<(u32, u32)>>,
}

/// Start of the data segment layout: heap pointer, heap end, then user data.
pub fn heap_ptr() -> u64 {
    elf::data_base()
}
pub fn heap_end() -> u64 {
    elf::data_base() + 8
}
pub fn sp_cell() -> u64 {
    elf::data_base() + 16
}
/// Word-sized scratch cells (index 0 to 13) for the native std (`__global_get`/`__global_set`).
pub fn globals() -> u64 {
    elf::data_base() + 24
}
pub fn data_start() -> u64 {
    elf::data_base() + 128
}

impl Backend {
    pub fn new() -> Backend {
        let mut b = settings::builder();
        b.set("opt_level", "speed").unwrap();
        b.set("is_pic", "false").unwrap();
        b.set("preserve_frame_pointers", "false").unwrap();
        let flags = settings::Flags::new(b);
        let triple = target_lexicon::Triple::from_str("x86_64-unknown-linux-gnu").unwrap();
        let mut ib = isa::lookup(triple).unwrap();
        // Any x86-64 CPU from the last fifteen years has these (needed for round/floor/ceil/trunc).
        for f in ["has_sse3", "has_ssse3", "has_sse41", "has_sse42"] {
            ib.set(f, "true").unwrap();
        }
        let isa = ib.finish(flags).unwrap();
        Backend { isa, code: vec![], relocs: vec![], data: vec![0; (data_start() - elf::data_base()) as usize], debug: None, names: Default::default(), locs: vec![] }
    }

    pub fn call_conv(&self) -> CallConv {
        CallConv::SystemV
    }

    pub fn sig(&self, nparams: usize, nrets: usize) -> ir::Signature {
        let mut s = ir::Signature::new(self.call_conv());
        for _ in 0..nparams {
            s.params.push(ir::AbiParam::new(ir::types::I64));
        }
        for _ in 0..nrets {
            s.returns.push(ir::AbiParam::new(ir::types::I64));
        }
        s
    }

    pub fn user_name(id: u32) -> UserExternalName {
        UserExternalName::new(0, id)
    }

    /// Add bytes to the data segment; returns the absolute address.
    pub fn add_data(&mut self, bytes: &[u8], align: usize) -> u64 {
        while self.data.len() % align != 0 {
            self.data.push(0);
        }
        let addr = elf::data_base() + self.data.len() as u64;
        self.data.extend_from_slice(bytes);
        addr
    }

    /// Define a function from raw machine code (an assembled `asm` block).
    pub fn define_bytes(&mut self, id: u32, bytes: Vec<u8>) {
        let i = id as usize;
        if self.code.len() <= i {
            self.code.resize(i + 1, None);
            self.relocs.resize(i + 1, vec![]);
        }
        if self.locs.len() <= i {
            self.locs.resize(i + 1, vec![]);
        }
        self.code[i] = Some(bytes);
        self.relocs[i] = vec![];
    }

    pub fn define(&mut self, id: u32, func: Function) -> Result<(), String> {
        let names: Vec<(cranelift_codegen::ir::UserExternalNameRef, UserExternalName)> =
            func.params.user_named_funcs().iter().map(|(k, v)| (k, v.clone())).collect();
        let mut ctx = Context::for_function(func);
        let mut ctrl = ControlPlane::default();
        let compiled = ctx.compile(&*self.isa, &mut ctrl).map_err(|e| format!("codegen: {:?}", e.inner))?;
        let bytes = compiled.code_buffer().to_vec();
        let mut rows: Vec<(u32, u32)> = vec![];
        if self.debug.is_some() {
            for l in compiled.buffer.get_srclocs_sorted() {
                if !l.loc.is_default() {
                    rows.push((l.start, l.loc.bits()));
                }
            }
        }
        let mut rs = vec![];
        for r in compiled.buffer.relocs() {
            let FinalizedRelocTarget::ExternalName(ExternalName::User(u)) = &r.target else {
                return Err(format!("unsupported relocation target {:?}", r.target));
            };
            let name = &names.iter().find(|(k, _)| k == u).unwrap().1;
            match r.kind {
                cranelift_codegen::binemit::Reloc::X86CallPCRel4 => rs.push((r.offset, name.index, r.addend, false)),
                cranelift_codegen::binemit::Reloc::Abs8 => rs.push((r.offset, name.index, r.addend, true)),
                k => return Err(format!("unsupported relocation {k:?}")),
            }
        }
        let i = id as usize;
        if self.code.len() <= i {
            self.code.resize(i + 1, None);
            self.relocs.resize(i + 1, vec![]);
        }
        if self.locs.len() <= i {
            self.locs.resize(i + 1, vec![]);
        }
        self.locs[i] = rows;
        self.code[i] = Some(bytes);
        self.relocs[i] = rs;
        Ok(())
    }

    /// Link everything into an ELF image. `entry` is called with the initial stack pointer
    /// (pointing at argc) and its return value is the exit status.
    pub fn link(&self, entry: u32, bss: u64) -> Result<Vec<u8>, String> {
        // _start: map a 1 GiB lazily committed stack (falling back to the kernel's), then
        // `call entry(sp)` and exit with its result.
        let mut start_pre: Vec<u8> = vec![
            0x31, 0xed, // xor ebp, ebp
            0x49, 0x89, 0xe4, // mov r12, rsp
            0xb8, 0x09, 0, 0, 0, // mov eax, 9 (mmap)
            0x31, 0xff, // xor edi, edi
            0x48, 0xc7, 0xc6, 0, 0, 0, 0x40, // mov rsi, 1 GiB
            0xba, 0x03, 0, 0, 0, // mov edx, PROT_READ|PROT_WRITE
            0x41, 0xba, 0x22, 0x40, 0, 0, // mov r10d, MAP_PRIVATE|MAP_ANONYMOUS|MAP_NORESERVE
            0x49, 0xc7, 0xc0, 0xff, 0xff, 0xff, 0xff, // mov r8, -1
            0x45, 0x31, 0xc9, // xor r9d, r9d
            0x0f, 0x05, // syscall
            0x48, 0x3d, 0x00, 0xf0, 0xff, 0xff, // cmp rax, -4096
            0x77, 0x07, // ja +7
            0x48, 0x8d, 0xa0, 0, 0, 0, 0x40, // lea rsp, [rax + 1 GiB]
            0x4c, 0x89, 0xe7, // mov rdi, r12
            0x48, 0x83, 0xe4, 0xf0, // and rsp, -16
            0xe8, // call rel32
        ];
        if elf::lythos() {
            // Lythos: the kernel provides the stack; no mmap (number 9 is a different syscall there).
            start_pre = vec![
                0x31, 0xed, // xor ebp, ebp
                0x48, 0x89, 0xe7, // mov rdi, rsp
                0x48, 0x83, 0xe4, 0xf0, // and rsp, -16
                0xe8, // call rel32
            ];
        }
        let exit_nr: u8 = if elf::lythos() { 1 } else { 60 };
        let start_post: Vec<u8> = vec![0x48, 0x89, 0xc7, 0xb8, exit_nr, 0, 0, 0, 0x0f, 0x05];
        let start_len = start_pre.len() + 4 + start_post.len();
        // syscall(n, a1..a6): rax=n, rdi=a1, rsi=a2, rdx=a3, r10=a4, r8=a5, r9=a6 (read from the stack).
        let stub: &[u8] = &[
            0x48, 0x89, 0xf8, 0x48, 0x89, 0xf7, 0x48, 0x89, 0xd6, 0x48, 0x89, 0xca, 0x4d, 0x89, 0xc2, 0x4d, 0x89, 0xc8,
            0x4c, 0x8b, 0x4c, 0x24, 0x08, 0x0f, 0x05, 0xc3,
        ];
        let mut text: Vec<u8> = vec![0; start_len];
        let pad = |t: &mut Vec<u8>| while t.len() % 16 != 0 {
            t.push(0xcc);
        };
        pad(&mut text);
        let stub_off = text.len();
        text.extend_from_slice(stub);
        pad(&mut text);
        let mut offs = vec![0usize; self.code.len()];
        for (i, c) in self.code.iter().enumerate() {
            if let Some(c) = c {
                offs[i] = text.len();
                text.extend_from_slice(c);
                pad(&mut text);
            }
        }
        let target = |t: u32| -> Result<usize, String> {
            if t == SYSCALL {
                Ok(stub_off)
            } else if (t as usize) < self.code.len() && self.code[t as usize].is_some() {
                Ok(offs[t as usize])
            } else {
                Err(format!("undefined function #{t}"))
            }
        };
        for (i, rs) in self.relocs.iter().enumerate() {
            for &(off, t, addend, abs) in rs {
                let at = offs[i] + off as usize;
                if abs {
                    let a = elf::text_addr() as i64 + target(t)? as i64 + addend;
                    text[at..at + 8].copy_from_slice(&a.to_le_bytes());
                } else {
                    let rel = target(t)? as i64 + addend - at as i64;
                    text[at..at + 4].copy_from_slice(&(rel as i32).to_le_bytes());
                }
            }
        }
        // _start
        let e = target(entry)?;
        let rel = e as i64 - (start_pre.len() as i64 + 4);
        start_pre.extend_from_slice(&(rel as i32).to_le_bytes());
        start_pre.extend_from_slice(&start_post);
        text[..start_pre.len()].copy_from_slice(&start_pre);
        let mut extra = vec![];
        if let Some(files) = &self.debug {
            let text_va = elf::text_addr();
            let mut funcs = vec![];
            for (i, c) in self.code.iter().enumerate() {
                let (Some(c), Some(name)) = (c, self.names.get(&(i as u32))) else { continue };
                let addr = text_va + offs[i] as u64;
                let locs = self.locs.get(i).map(|rows| rows.iter().map(|&(o, l)| (addr + o as u64, l)).collect()).unwrap_or_default();
                funcs.push(crate::dwarf::FuncInfo { name: name.clone(), addr, len: c.len() as u64, locs });
            }
            funcs.sort_by_key(|f| f.addr);
            extra = crate::dwarf::sections(files, &funcs, text_va, text_va + text.len() as u64);
        }
        let img = elf::Image { text: &text, entry: 0, data: &self.data, bss, extra };
        Ok(elf::write(&img))
    }
}

impl Default for Backend {
    fn default() -> Self {
        Backend::new()
    }
}
