//! Builtins that the lowering emits directly: numeric functions, regions and allocator names.

use crate::lower::*;
use crate::rt;
use cera_sema::ty::Ty;
use cera_syntax::ast::*;
use cranelift_codegen::ir::{self, condcodes::FloatCC, condcodes::IntCC, types::F64, types::I64, InstBuilder, MemFlags};

type R<T> = Result<T, String>;

fn is_float(t: &Ty) -> bool {
    is_con(t, "f64") || is_con(t, "f32")
}

impl<'c, 'a, 'b> FnCx<'c, 'a, 'b> {
    pub(crate) fn fbits_to_f64(&mut self, v: ir::Value) -> ir::Value {
        self.b.ins().bitcast(F64, MemFlags::new(), v)
    }

    pub(crate) fn f64_to_bits(&mut self, v: ir::Value) -> ir::Value {
        self.b.ins().bitcast(I64, MemFlags::new(), v)
    }

    /// `promote`, `ref_of`, `new_in` and `arena_new` (regions have no runtime effect yet).
    pub(crate) fn core_builtin(&mut self, name: &str, args: &[&Expr]) -> R<Option<ir::Value>> {
        match (name, args.len()) {
            ("promote", 1) | ("ref_of", 1) => Ok(Some(self.expr(args[0], false)?)),
            ("new_in", 2) => {
                self.expr(args[0], false)?;
                Ok(Some(self.expr(args[1], false)?))
            }
            ("arena_new", 1) => {
                self.expr(args[0], false)?;
                Ok(Some(self.iconst(0)))
            }
            _ => Ok(None),
        }
    }

    /// `bits.*`: bitwise operations and wrapping arithmetic on integers of any width.
    pub(crate) fn bits_builtin(&mut self, name: &str, args: &[&Expr]) -> R<Option<ir::Value>> {
        if !matches!(name, "band" | "bor" | "bxor" | "bnot" | "shl" | "shr" | "rotl" | "rotr" | "popcount" | "leading_zeros" | "trailing_zeros" | "wrapping_add" | "wrapping_sub" | "wrapping_mul") {
            return Ok(None);
        }
        let t = self.ty_of(args[0])?;
        let (bits, signed) = int_kind(&t).ok_or_else(|| format!("`bits.{name}` needs integers"))?;
        let x = self.expr(args[0], false)?;
        let y = if args.len() > 1 { Some(self.expr(args[1], false)?) } else { None };
        let norm = |cx: &mut Self, v: ir::Value| -> ir::Value {
            if bits == 64 {
                v
            } else if signed {
                let a = cx.b.ins().ishl_imm(v, (64 - bits) as i64);
                cx.b.ins().sshr_imm(a, (64 - bits) as i64)
            } else {
                cx.b.ins().band_imm(v, ((1u64 << bits) - 1) as i64)
            }
        };
        let raw = |cx: &mut Self, v: ir::Value| -> ir::Value {
            if bits == 64 || !signed {
                v
            } else {
                cx.b.ins().band_imm(v, ((1u64 << bits) - 1) as i64)
            }
        };
        let r = match name {
            "band" => self.b.ins().band(x, y.unwrap()),
            "bor" => self.b.ins().bor(x, y.unwrap()),
            "bxor" => self.b.ins().bxor(x, y.unwrap()),
            "bnot" => {
                let v = self.b.ins().bnot(x);
                norm(self, v)
            }
            "shl" | "shr" => {
                let n = y.unwrap();
                let bad = self.b.ins().icmp_imm(IntCC::UnsignedGreaterThanOrEqual, n, bits as i64);
                self.panic_if(bad, &format!("shift amount is not less than the width of a {bits}-bit integer"));
                if name == "shl" {
                    let v = self.b.ins().ishl(x, n);
                    norm(self, v)
                } else if signed {
                    self.b.ins().sshr(x, n)
                } else {
                    self.b.ins().ushr(x, n)
                }
            }
            "rotl" | "rotr" => {
                let k = self.b.ins().urem_imm(y.unwrap(), bits as i64);
                let u = raw(self, x);
                if bits == 64 {
                    if name == "rotl" { self.b.ins().rotl(u, k) } else { self.b.ins().rotr(u, k) }
                } else {
                    let comp = {
                        let w = self.iconst(bits as i64);
                        self.b.ins().isub(w, k)
                    };
                    let (a, b) = if name == "rotl" { (self.b.ins().ishl(u, k), self.b.ins().ushr(u, comp)) } else { (self.b.ins().ushr(u, k), self.b.ins().ishl(u, comp)) };
                    let v = self.b.ins().bor(a, b);
                    norm(self, v)
                }
            }
            "popcount" => {
                let u = raw(self, x);
                self.b.ins().popcnt(u)
            }
            "leading_zeros" => {
                let u = raw(self, x);
                let c = self.b.ins().clz(u);
                self.b.ins().iadd_imm(c, -((64 - bits) as i64))
            }
            "trailing_zeros" => {
                let u = raw(self, x);
                let c = self.b.ins().ctz(u);
                let zero = self.b.ins().icmp_imm(IntCC::Equal, u, 0);
                let w = self.iconst(bits as i64);
                self.b.ins().select(zero, w, c)
            }
            _ => {
                let v = match name {
                    "wrapping_add" => self.b.ins().iadd(x, y.unwrap()),
                    "wrapping_sub" => self.b.ins().isub(x, y.unwrap()),
                    _ => self.b.ins().imul(x, y.unwrap()),
                };
                norm(self, v)
            }
        };
        Ok(Some(r))
    }

    pub(crate) fn num_builtin(&mut self, name: &str, args: &[&Expr], whole: &Expr) -> R<Option<ir::Value>> {
        let float_fn = |n: &str| matches!(n, "sqrt" | "floor" | "ceil" | "round");
        if !(float_fn(name) || matches!(name, "abs" | "pow" | "to_f64" | "to_i8" | "to_i16" | "to_i32" | "to_i64" | "to_isize" | "to_u8" | "to_u16" | "to_u32" | "to_u64" | "to_usize")) {
            return Ok(None);
        }
        let t = self.ty_of(args[0])?;
        let x = self.expr(args[0], false)?;
        let f32ty = is_con(&t, "f32");
        match name {
            "sqrt" | "floor" | "ceil" => {
                let f = self.fbits_to_f64(x);
                let r = match name {
                    "sqrt" => self.b.ins().sqrt(f),
                    "floor" => self.b.ins().floor(f),
                    _ => self.b.ins().ceil(f),
                };
                Ok(Some(self.f64_to_bits(r)))
            }
            "round" => {
                let f = self.fbits_to_f64(x);
                let tr = self.b.ins().trunc(f);
                let d = self.b.ins().fsub(f, tr);
                let d = self.b.ins().fabs(d);
                let half = self.b.ins().f64const(0.5);
                let big = self.b.ins().fcmp(FloatCC::GreaterThanOrEqual, d, half);
                let one = self.b.ins().f64const(1.0);
                let step = self.b.ins().fcopysign(one, f);
                let up = self.b.ins().fadd(tr, step);
                let r = self.b.ins().select(big, up, tr);
                Ok(Some(self.f64_to_bits(r)))
            }
            "abs" => {
                if is_float(&t) {
                    let f = self.fbits_to_f64(x);
                    let r = self.b.ins().fabs(f);
                    return Ok(Some(self.f64_to_bits(r)));
                }
                let (bits, signed) = int_kind(&t).ok_or("abs needs a number")?;
                if !signed {
                    return Ok(Some(x));
                }
                let neg = self.b.ins().icmp_imm(IntCC::SignedLessThan, x, 0);
                let nx = self.b.ins().ineg(x);
                let bad = if bits == 64 {
                    self.b.ins().icmp_imm(IntCC::Equal, x, i64::MIN)
                } else {
                    let sh = (64 - bits) as i64;
                    let a = self.b.ins().ishl_imm(nx, sh);
                    let c = self.b.ins().sshr_imm(a, sh);
                    self.b.ins().icmp(IntCC::NotEqual, c, nx)
                };
                self.panic_if(bad, "integer overflow in `abs`");
                Ok(Some(self.b.ins().select(neg, nx, x)))
            }
            "pow" => {
                let e = self.expr(args[1], false)?;
                if is_float(&t) {
                    let eb = e;
                    let r = self.call_std("float_powi", &[], &[x, eb])?;
                    let r = if f32ty {
                        let f = self.fbits_to_f64(r);
                        let s = self.b.ins().fdemote(ir::types::F32, f);
                        let p = self.b.ins().fpromote(F64, s);
                        self.f64_to_bits(p)
                    } else {
                        r
                    };
                    return Ok(Some(r));
                }
                int_kind(&t).ok_or("pow needs a number")?;
                // acc = 1; repeat e times: acc = acc * x
                let lp = self.b.create_block();
                self.b.append_block_param(lp, I64);
                self.b.append_block_param(lp, I64);
                let done = self.b.create_block();
                self.b.append_block_param(done, I64);
                let one = self.iconst(1);
                self.b.ins().jump(lp, &[one.into(), e.into()]);
                self.b.switch_to_block(lp);
                let (acc, n) = (self.b.block_params(lp)[0], self.b.block_params(lp)[1]);
                let body = self.b.create_block();
                self.b.ins().brif(n, body, &[], done, &[acc.into()]);
                self.b.switch_to_block(body);
                let nacc = self.binary_values(BinOp::Mul, &t, acc, x)?;
                let n2 = self.b.ins().iadd_imm(n, -1);
                self.b.ins().jump(lp, &[nacc.into(), n2.into()]);
                self.b.switch_to_block(done);
                Ok(Some(self.b.block_params(done)[0]))
            }
            "to_f64" => {
                if is_float(&t) {
                    return Ok(Some(x));
                }
                let (_, signed) = int_kind(&t).ok_or("to_f64 needs a number")?;
                let f = if signed { self.b.ins().fcvt_from_sint(F64, x) } else { self.b.ins().fcvt_from_uint(F64, x) };
                Ok(Some(self.f64_to_bits(f)))
            }
            _ => {
                // to_i32 ... to_usize: an Option of the converted value.
                let (tbits, tsigned): (u32, bool) = match name {
                    "to_i8" => (8, true),
                    "to_i16" => (16, true),
                    "to_i32" => (32, true),
                    "to_i64" | "to_isize" => (64, true),
                    "to_u8" => (8, false),
                    "to_u16" => (16, false),
                    "to_u32" => (32, false),
                    _ => (64, false),
                };
                let (rlo, rhi): (i128, i128) = if tsigned { (-(1i128 << (tbits - 1)), (1i128 << (tbits - 1)) - 1) } else { (0, (1i128 << tbits) - 1) };
                let _ = whole;
                let (ok, v) = if is_float(&t) {
                    let f = self.fbits_to_f64(x);
                    let tr = self.b.ins().trunc(f);
                    // in range: lo <= tr < hi_excl (all bounds are exactly representable)
                    let (lo, hi): (f64, f64) = (rlo as f64, (rhi + 1) as f64);
                    let (lov, hiv) = (self.b.ins().f64const(lo), self.b.ins().f64const(hi));
                    let a = self.b.ins().fcmp(FloatCC::GreaterThanOrEqual, tr, lov);
                    let c = self.b.ins().fcmp(FloatCC::LessThan, tr, hiv);
                    let ok = self.b.ins().band(a, c);
                    let ok = self.b.ins().uextend(I64, ok);
                    // Convert only when in range, otherwise use a safe operand.
                    let safe = self.b.ins().select(ok, tr, lov);
                    let v = if tsigned || tbits < 64 { self.b.ins().fcvt_to_sint_sat(I64, safe) } else { self.b.ins().fcvt_to_uint_sat(I64, safe) };
                    (ok, v)
                } else {
                    let (sbits, ssigned) = int_kind(&t).ok_or("conversion needs a number")?;
                    let _ = sbits;
                    let (lo, hi) = (rlo, rhi);
                    let ok = if ssigned {
                        // x is a sign-extended signed value
                        let a = if lo == 0 { self.b.ins().icmp_imm(IntCC::SignedGreaterThanOrEqual, x, 0) } else { self.b.ins().icmp_imm(IntCC::SignedGreaterThanOrEqual, x, lo as i64) };
                        let c = if hi >= i64::MAX as i128 { self.b.ins().iconst(ir::types::I8, 1) } else { self.b.ins().icmp_imm(IntCC::SignedLessThanOrEqual, x, hi as i64) };
                        self.b.ins().band(a, c)
                    } else if hi >= u64::MAX as i128 {
                        self.b.ins().iconst(ir::types::I8, 1)
                    } else {
                        self.b.ins().icmp_imm(IntCC::UnsignedLessThanOrEqual, x, hi as i64)
                    };
                    let ok = self.b.ins().uextend(I64, ok);
                    (ok, x)
                };
                // Option layout: Some = [0, v], None = static [1].
                let some_blk = self.b.create_block();
                let none_blk = self.b.create_block();
                let merge = self.b.create_block();
                self.b.append_block_param(merge, I64);
                self.b.ins().brif(ok, some_blk, &[], none_blk, &[]);
                self.b.switch_to_block(some_blk);
                let cell = self.alloc(2);
                let tag = self.iconst(0);
                self.store_words(cell, 0, &[tag, v]);
                self.b.ins().jump(merge, &[cell.into()]);
                self.b.switch_to_block(none_blk);
                let (id, idx) = self.c.ctor("None").ok_or("Option is missing")?;
                let a = self.c.nullary_addr(&id, idx);
                let none = self.iconst(a as i64);
                self.b.ins().jump(merge, &[none.into()]);
                self.b.switch_to_block(merge);
                Ok(Some(self.b.block_params(merge)[0]))
            }
        }
    }

    /// Raw memory and syscall intrinsics used by the native std.
    pub(crate) fn raw_intrinsic(&mut self, name: &str, vs: &[ir::Value]) -> R<Option<ir::Value>> {
        use crate::backend::*;
        Ok(Some(match name {
            "__syscall6" => rt::call(&mut self.b, &self.c.be, SYSCALL, vs.len(), vs)[0],
            "__lythos" => self.iconst(crate::elf::lythos() as i64),
            "__syscall" => rt::call(&mut self.b, &self.c.be, SYSCALL, 4, vs)[0],
            "__peek" => self.b.ins().load(I64, MemFlags::new(), vs[0], 0),
            "__peek16" => self.b.ins().uload16(I64, MemFlags::new(), vs[0], 0),
            "__peek32" => self.b.ins().uload32(MemFlags::new(), vs[0], 0),
            "__poke8" => {
                self.b.ins().istore8(MemFlags::new(), vs[1], vs[0], 0);
                self.iconst(0)
            }
            "__poke16" => {
                self.b.ins().istore16(MemFlags::new(), vs[1], vs[0], 0);
                self.iconst(0)
            }
            "__poke32" => {
                self.b.ins().istore32(MemFlags::new(), vs[1], vs[0], 0);
                self.iconst(0)
            }
            "__peek_byte" => self.b.ins().uload8(I64, MemFlags::new(), vs[0], 0),
            "__poke" => {
                self.b.ins().store(MemFlags::new(), vs[1], vs[0], 0);
                self.iconst(0)
            }
            "__sp" => {
                let a = self.iconst(sp_cell() as i64);
                self.b.ins().load(I64, MemFlags::new(), a, 0)
            }
            "__heap_base" => self.iconst(heap_ptr() as i64),
            "__global_get" => {
                let off = self.b.ins().ishl_imm(vs[0], 3);
                let base = self.iconst(globals() as i64);
                let a = self.b.ins().iadd(base, off);
                self.b.ins().load(I64, MemFlags::new(), a, 0)
            }
            "__global_set" => {
                let off = self.b.ins().ishl_imm(vs[0], 3);
                let base = self.iconst(globals() as i64);
                let a = self.b.ins().iadd(base, off);
                self.b.ins().store(MemFlags::new(), vs[1], a, 0);
                self.iconst(0)
            }
            "__payload" => self.b.ins().iadd_imm(vs[0], 8),
            "__sqrt" | "__floor" | "__ceil" | "__trunc" => {
                let f = self.fbits_to_f64(vs[0]);
                let r = match name {
                    "__sqrt" => self.b.ins().sqrt(f),
                    "__floor" => self.b.ins().floor(f),
                    "__ceil" => self.b.ins().ceil(f),
                    _ => self.b.ins().trunc(f),
                };
                self.f64_to_bits(r)
            }
            _ => return Ok(None),
        }))
    }
}
