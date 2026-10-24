//! Runtime support functions, generated through Cranelift (no libc, raw syscalls).

use crate::backend::*;
use crate::elf;
use cranelift_codegen::ir::{self, condcodes::IntCC, types::I64, types::I8, InstBuilder, MemFlags, StackSlotData, StackSlotKind, UserFuncName};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};

pub const RT_WRITE: u32 = 0; // (fd, ptr, len)
pub const RT_EXIT: u32 = 1; // (code)
pub const RT_PRINT_INT: u32 = 2; // (fd, n, signed, newline)
pub const RT_PRINT_STR: u32 = 3; // (fd, s, newline)
pub const RT_GROW: u32 = 4; // (size) -> ptr
pub const RT_PANIC: u32 = 5; // (msg string)
pub const RT_START: u32 = 6;
pub const RT_MEMCPY: u32 = 7; // (dst, src, n)
pub const FIRST_USER: u32 = 16;

pub fn call(b: &mut FunctionBuilder, be: &Backend, id: u32, nparams: usize, args: &[ir::Value]) -> Vec<ir::Value> {
    let sig = be.sig(nparams, 1);
    let sr = b.import_signature(sig);
    let nm = b.func.declare_imported_user_function(Backend::user_name(id));
    let fr = b.import_function(ir::ExtFuncData { name: ir::ExternalName::user(nm), signature: sr, colocated: true });
    let c = b.ins().call(fr, args);
    b.inst_results(c).to_vec()
}

fn syscall(b: &mut FunctionBuilder, be: &Backend, args: &[ir::Value]) -> ir::Value {
    let sig = be.sig(args.len(), 1);
    let sr = b.import_signature(sig);
    let nm = b.func.declare_imported_user_function(Backend::user_name(SYSCALL));
    let fr = b.import_function(ir::ExtFuncData { name: ir::ExternalName::user(nm), signature: sr, colocated: true });
    let c = b.ins().call(fr, args);
    b.inst_results(c)[0]
}

fn build(be: &mut Backend, id: u32, nparams: usize, f: impl FnOnce(&mut FunctionBuilder, &Backend, &[ir::Value])) -> Result<(), String> {
    let sig = be.sig(nparams, 1);
    let mut func = ir::Function::with_name_signature(UserFuncName::user(0, id), sig);
    let mut fc = FunctionBuilderContext::new();
    {
        let mut b = FunctionBuilder::new(&mut func, &mut fc);
        let e = b.create_block();
        b.append_block_params_for_function_params(e);
        b.switch_to_block(e);
        let ps = b.block_params(e).to_vec();
        f(&mut b, be, &ps);
        b.seal_all_blocks();
        b.finalize();
    }
    be.define(id, func)
}

/// Define all runtime functions. `start_main` is the id of the user `main` instance, and
/// `main_returns_int` says whether its result is the exit status.
pub fn define_all(be: &mut Backend, main: u32, main_params: usize, main_returns_int: bool, newline: u64) -> Result<(), String> {
    // The `Caps` record: console, fs, clock, env. Only `fs` carries a value: the root directory
    // (AT_FDCWD on Linux; the path "/" on Lythos). See `native_std.cera`.
    let root: u64 = if elf::lythos() {
        be.add_data(&[&1u64.to_le_bytes()[..], b"/\0\0\0\0\0\0\0"].concat(), 8)
    } else {
        (-100i64 + 1024) as u64 * 2
    };
    let mut blob = [0u8; 32];
    blob[8..16].copy_from_slice(&root.to_le_bytes());
    let caps = be.add_data(&blob, 8);
    build(be, RT_WRITE, 3, |b, be, p| {
        if elf::lythos() {
            // Standard streams go to the kernel console (SYS_LOG); other descriptors use SYS_WRITE.
            let console = b.create_block();
            let file = b.create_block();
            let is_std = b.ins().icmp_imm(IntCC::UnsignedLessThanOrEqual, p[0], 2);
            b.ins().brif(is_std, console, &[], file, &[]);
            b.switch_to_block(console);
            let n = b.ins().iconst(I64, 11);
            syscall(b, be, &[n, p[1], p[2]]);
            b.ins().return_(&[p[2]]);
            b.switch_to_block(file);
            let n = b.ins().iconst(I64, 24);
            let r = syscall(b, be, &[n, p[0], p[1], p[2]]);
            b.ins().return_(&[r]);
        } else {
            let n = b.ins().iconst(I64, 1);
            let r = syscall(b, be, &[n, p[0], p[1], p[2]]);
            b.ins().return_(&[r]);
        }
    })?;
    build(be, RT_EXIT, 1, |b, be, p| {
        let n = b.ins().iconst(I64, if elf::lythos() { 1 } else { 60 });
        syscall(b, be, &[n, p[0]]);
        b.ins().trap(ir::TrapCode::unwrap_user(1));
    })?;
    // print_str(fd, s, newline)
    build(be, RT_PRINT_STR, 3, |b, be, p| {
        let len = b.ins().load(I64, MemFlags::trusted(), p[1], 0);
        let data = b.ins().iadd_imm(p[1], 8);
        call(b, be, RT_WRITE, 3, &[p[0], data, len]);
        let done = b.create_block();
        let nl = b.create_block();
        b.ins().brif(p[2], nl, &[], done, &[]);
        b.switch_to_block(nl);
        let a = b.ins().iconst(I64, newline as i64);
        let one = b.ins().iconst(I64, 1);
        call(b, be, RT_WRITE, 3, &[p[0], a, one]);
        b.ins().jump(done, &[]);
        b.switch_to_block(done);
        let z = b.ins().iconst(I64, 0);
        b.ins().return_(&[z]);
    })?;
    // print_int(fd, n, signed, newline)
    build(be, RT_PRINT_INT, 4, |b, be, p| {
        let slot = b.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, 48, 3));
        let base = b.ins().stack_addr(I64, slot, 0);
        // neg = signed && n < 0
        let zero = b.ins().iconst(I64, 0);
        let lt = b.ins().icmp(IntCC::SignedLessThan, p[1], zero);
        let lt = b.ins().uextend(I64, lt);
        let neg = b.ins().band(lt, p[2]);
        let negv = b.ins().ineg(p[1]);
        let mag0 = b.ins().select(neg, negv, p[1]);
        // write digits backwards from base+40
        let end = b.ins().iadd_imm(base, 40);
        let lp = b.create_block();
        b.append_block_param(lp, I64); // value
        b.append_block_param(lp, I64); // cursor
        b.ins().jump(lp, &[mag0.into(), end.into()]);
        b.switch_to_block(lp);
        let v = b.block_params(lp)[0];
        let cur = b.block_params(lp)[1];
        let ten = b.ins().iconst(I64, 10);
        let q = b.ins().udiv(v, ten);
        let r = b.ins().urem(v, ten);
        let ch = b.ins().iadd_imm(r, 48);
        let ncur = b.ins().iadd_imm(cur, -1);
        let ch8 = b.ins().ireduce(I8, ch);
        b.ins().store(MemFlags::trusted(), ch8, ncur, 0);
        let more = b.create_block();
        let fin = b.create_block();
        b.append_block_param(fin, I64);
        b.ins().brif(q, more, &[], fin, &[ncur.into()]);
        b.switch_to_block(more);
        b.ins().jump(lp, &[q.into(), ncur.into()]);
        b.switch_to_block(fin);
        let cur2 = b.block_params(fin)[0];
        // sign
        let sgn = b.create_block();
        let after = b.create_block();
        b.append_block_param(after, I64);
        b.ins().brif(neg, sgn, &[], after, &[cur2.into()]);
        b.switch_to_block(sgn);
        let c3 = b.ins().iadd_imm(cur2, -1);
        let minus = b.ins().iconst(ir::types::I8, 45);
        b.ins().store(MemFlags::trusted(), minus, c3, 0);
        b.ins().jump(after, &[c3.into()]);
        b.switch_to_block(after);
        let start = b.block_params(after)[0];
        // newline at end
        let nlb = b.ins().iconst(ir::types::I8, 10);
        b.ins().store(MemFlags::trusted(), nlb, end, 0);
        let len0 = b.ins().isub(end, start);
        let len1 = b.ins().iadd_imm(len0, 1);
        let len = b.ins().select(p[3], len1, len0);
        call(b, be, RT_WRITE, 3, &[p[0], start, len]);
        let z = b.ins().iconst(I64, 0);
        b.ins().return_(&[z]);
    })?;
    // grow(size): map a fresh chunk, bump from it.
    build(be, RT_GROW, 1, |b, be, p| {
        let chunk = b.ins().iconst(I64, if elf::lythos() { 1 << 20 } else { 64 << 20 });
        let big = b.ins().icmp(IntCC::UnsignedGreaterThan, p[0], chunk);
        let sz0 = b.ins().select(big, p[0], chunk);
        let sz = b.ins().iadd_imm(sz0, 4095);
        let sz = b.ins().band_imm(sz, !4095);
        let zero = b.ins().iconst(I64, 0);
        let (r, ok) = if elf::lythos() {
            // Grow the program break: query it, then move it up by `sz`.
            let n = b.ins().iconst(I64, 38);
            let cur = syscall(b, be, &[n, zero]);
            let want = b.ins().iadd(cur, sz);
            let n = b.ins().iconst(I64, 38);
            let got = syscall(b, be, &[n, want]);
            let diff = b.ins().isub(got, want);
            let ok = b.ins().icmp_imm(IntCC::UnsignedLessThan, diff, 4096);
            (cur, ok)
        } else {
            let n = b.ins().iconst(I64, 9);
            let prot = b.ins().iconst(I64, 3);
            let flags = b.ins().iconst(I64, 0x22 | 0x4000); // PRIVATE|ANONYMOUS|NORESERVE
            let fd = b.ins().iconst(I64, -1);
            let r = syscall(b, be, &[n, zero, sz, prot, flags, fd, zero]);
            (r, b.ins().icmp_imm(IntCC::SignedGreaterThan, r, 0))
        };
        let fail = b.create_block();
        let good = b.create_block();
        b.ins().brif(ok, good, &[], fail, &[]);
        b.switch_to_block(fail);
        let msg = b.ins().iconst(I64, 0);
        call(b, be, RT_PANIC, 1, &[msg]);
        b.ins().trap(ir::TrapCode::unwrap_user(2));
        b.switch_to_block(good);
        let np = b.ins().iadd(r, p[0]);
        let np = b.ins().iadd_imm(np, 0);
        let hp = b.ins().iconst(I64, heap_ptr() as i64);
        b.ins().store(MemFlags::trusted(), np, hp, 0);
        let cs = b.ins().iconst(I64, (globals() + 48) as i64);
        b.ins().store(MemFlags::trusted(), r, cs, 0);
        let he = b.ins().iconst(I64, heap_end() as i64);
        let e = b.ins().iadd(r, sz);
        b.ins().store(MemFlags::trusted(), e, he, 0);
        b.ins().return_(&[r]);
    })?;
    // panic(msg): msg (already complete, ends in a newline) to stderr, exit 1. A null msg means out of memory.
    let oom = be.add_data(&[&29u64.to_le_bytes()[..], b"runtime error: out of memory\n"].concat(), 8);
    build(be, RT_PANIC, 1, |b, be, p| {
        let two = b.ins().iconst(I64, 2);
        let isnull = b.ins().icmp_imm(IntCC::Equal, p[0], 0);
        let o = b.ins().iconst(I64, oom as i64);
        let m = b.ins().select(isnull, o, p[0]);
        let zero = b.ins().iconst(I64, 0);
        call(b, be, RT_PRINT_STR, 3, &[two, m, zero]);
        let code = b.ins().iconst(I64, 1);
        call(b, be, RT_EXIT, 1, &[code]);
        b.ins().return_(&[code]);
    })?;
    // memcpy(dst, src, n): words first, then bytes
    build(be, RT_MEMCPY, 3, |b, _be, p| {
        let wl = b.create_block();
        b.append_block_param(wl, I64);
        b.append_block_param(wl, I64);
        b.append_block_param(wl, I64);
        b.ins().jump(wl, &[p[0].into(), p[1].into(), p[2].into()]);
        b.switch_to_block(wl);
        let (d, s, n) = (b.block_params(wl)[0], b.block_params(wl)[1], b.block_params(wl)[2]);
        let big = b.ins().icmp_imm(IntCC::UnsignedGreaterThanOrEqual, n, 8);
        let wbody = b.create_block();
        let bl = b.create_block();
        b.append_block_param(bl, I64);
        b.append_block_param(bl, I64);
        b.append_block_param(bl, I64);
        b.ins().brif(big, wbody, &[], bl, &[d.into(), s.into(), n.into()]);
        b.switch_to_block(wbody);
        let w = b.ins().load(I64, MemFlags::new(), s, 0);
        b.ins().store(MemFlags::new(), w, d, 0);
        let d2 = b.ins().iadd_imm(d, 8);
        let s2 = b.ins().iadd_imm(s, 8);
        let n2 = b.ins().iadd_imm(n, -8);
        b.ins().jump(wl, &[d2.into(), s2.into(), n2.into()]);
        b.switch_to_block(bl);
        let (d, s, n) = (b.block_params(bl)[0], b.block_params(bl)[1], b.block_params(bl)[2]);
        let more = b.ins().icmp_imm(IntCC::NotEqual, n, 0);
        let bbody = b.create_block();
        let done = b.create_block();
        b.ins().brif(more, bbody, &[], done, &[]);
        b.switch_to_block(bbody);
        let x = b.ins().uload8(I64, MemFlags::new(), s, 0);
        b.ins().istore8(MemFlags::new(), x, d, 0);
        let d2 = b.ins().iadd_imm(d, 1);
        let s2 = b.ins().iadd_imm(s, 1);
        let n2 = b.ins().iadd_imm(n, -1);
        b.ins().jump(bl, &[d2.into(), s2.into(), n2.into()]);
        b.switch_to_block(done);
        let z = b.ins().iconst(I64, 0);
        b.ins().return_(&[z]);
    })?;
    // start(sp) -> exit code
    build(be, RT_START, 1, |b, be, p| {
        let spc = b.ins().iconst(I64, sp_cell() as i64);
        b.ins().store(MemFlags::trusted(), p[0], spc, 0);
        // Stack limit for the overflow check: the whole stack (1 GiB on Linux, the kernel's
        // 256 KiB on Lythos) minus a margin for the runtime and syscalls.
        let now = b.ins().get_stack_pointer(I64);
        let budget = b.ins().iconst(I64, if elf::lythos() { 240 << 10 } else { (1 << 30) - (1 << 20) });
        let limit = b.ins().isub(now, budget);
        let lc = b.ins().iconst(I64, (globals() + 56) as i64);
        b.ins().store(MemFlags::trusted(), limit, lc, 0);
        let zero = b.ins().iconst(I64, 0);
        let args: Vec<ir::Value> = if main_params == 1 { vec![b.ins().iconst(I64, caps as i64)] } else { vec![] };
        let r = call(b, be, main, main_params, &args);
        let r = if main_returns_int { r[0] } else { zero };
        b.ins().return_(&[r]);
    })?;
    Ok(())
}
