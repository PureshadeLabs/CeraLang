//! Type-directed glue: `Show` (to a reversed list of string chunks) and structural equality.

use crate::lower::*;
use crate::rt;
use cera_sema::defs::TDef;
use cera_sema::ty::Ty;
use cranelift_codegen::ir::{self, condcodes::FloatCC, condcodes::IntCC, types::F64, types::I64, InstBuilder, MemFlags};
use std::collections::HashMap;

type R<T> = Result<T, String>;

pub(crate) fn compile_glue<'a>(c: &mut Compiler<'a>, id: u32, kind: GlueKind, ty: Ty) -> R<()> {
    match kind {
        GlueKind::Show { nested } => compile_custom(c, id, 2, |cx, p| cx.show_body(&ty, nested, p[0], p[1])),
        GlueKind::Eq => compile_custom(c, id, 2, |cx, p| cx.eq_body(&ty, p[0], p[1])),
        GlueKind::Lt => compile_custom(c, id, 2, |cx, p| cx.lt_body(&ty, p[0], p[1])),
    }
}

impl<'c, 'a, 'b> FnCx<'c, 'a, 'b> {
    /// Call a function of the native std module (non-generic unless `targs` is given).
    pub(crate) fn call_std(&mut self, name: &str, targs: &[Ty], args: &[ir::Value]) -> R<ir::Value> {
        let id = self.c.instance(1, name, targs)?;
        Ok(rt::call(&mut self.b, &self.c.be, id, args.len(), args)[0])
    }

    pub(crate) fn push_chunk(&mut self, s: ir::Value, acc: ir::Value) -> ir::Value {
        let cell = self.alloc(2);
        self.store_words(cell, 0, &[s, acc]);
        cell
    }

    pub(crate) fn push_static(&mut self, text: &str, acc: ir::Value) -> ir::Value {
        let a = self.c.string_addr(text);
        let s = self.iconst(a as i64);
        self.push_chunk(s, acc)
    }

    fn variant_fields(&self, id: &str, idx: usize, args: &[Ty]) -> R<(String, Vec<Ty>, usize)> {
        let ti = self.c.type_info(id)?;
        let TDef::Variants(vs) = &ti.def else { return Err("not a variant type".into()) };
        let m: HashMap<String, Ty> = ti.generics.iter().cloned().zip(args.iter().cloned()).collect();
        let fields = vs[idx].fields.iter().map(|t| norm(&subst_ty(t, &m))).collect();
        Ok((vs[idx].name.clone(), fields, vs.len()))
    }

    // ----- show --------------------------------------------------------------------

    fn show_sub(&mut self, ty: &Ty, v: ir::Value, acc: ir::Value) -> R<ir::Value> {
        let g = self.c.glue(GlueKind::Show { nested: true }, ty);
        Ok(rt::call(&mut self.b, &self.c.be, g, 2, &[v, acc])[0])
    }

    pub(crate) fn show_body(&mut self, ty: &Ty, nested: bool, v: ir::Value, acc: ir::Value) -> R<ir::Value> {
        // A user `impl Show` wins over the structural form.
        if matches!(ty, Ty::Con(..)) {
            if let Some((idx, sub)) = self.c.find_impl("show", ty) {
                let id = self.c.impl_instance(idx, &sub)?;
                let s = rt::call(&mut self.b, &self.c.be, id, 1, &[v])[0];
                return Ok(self.push_chunk(s, acc));
            }
        }
        match ty {
            Ty::Con(n, a) if a.is_empty() => {
                if let Some((_, signed)) = int_kind(ty) {
                    let s = self.call_std(if signed { "string_from_i64" } else { "string_from_u64" }, &[], &[v])?;
                    return Ok(self.push_chunk(s, acc));
                }
                match n.as_str() {
                    "bool" => {
                        let t = self.c.string_addr("true");
                        let f = self.c.string_addr("false");
                        let (tv, fv) = (self.iconst(t as i64), self.iconst(f as i64));
                        let s = self.b.ins().select(v, tv, fv);
                        Ok(self.push_chunk(s, acc))
                    }
                    "char" => {
                        let s = self.call_std(if nested { "string_quote_char" } else { "string_from_char" }, &[], &[v])?;
                        Ok(self.push_chunk(s, acc))
                    }
                    "String" => {
                        let s = if nested { self.call_std("string_quote", &[], &[v])? } else { v };
                        Ok(self.push_chunk(s, acc))
                    }
                    "f64" | "f32" => {
                        let s = self.call_std(if n == "f32" { "string_from_f32" } else { "string_from_f64" }, &[], &[v])?;
                        Ok(self.push_chunk(s, acc))
                    }
                    _ => self.show_adt(ty, v, acc),
                }
            }
            Ty::Tuple(ts) if ts.is_empty() => Ok(self.push_static("()", acc)),
            Ty::Tuple(ts) => {
                let mut acc = self.push_static("(", acc);
                for (i, t) in ts.iter().enumerate() {
                    if i > 0 {
                        acc = self.push_static(", ", acc);
                    }
                    let f = self.load_word(v, i);
                    acc = self.show_sub(t, f, acc)?;
                }
                Ok(self.push_static(")", acc))
            }
            Ty::List(e) => {
                let g = self.c.glue(GlueKind::Show { nested: true }, e);
                let clo = self.closure_of(g, 2);
                let acc = self.push_static("[", acc);
                let first = self.iconst(1);
                let acc = self.call_std("show_items", &[(**e).clone()], &[v, clo, first, acc])?;
                Ok(self.push_static("]", acc))
            }
            Ty::Ref(inner) => self.show_body(inner, nested, v, acc),
            Ty::Con(id, a) if id.ends_with("Array") && a.len() == 1 => {
                let items = self.call_std("array_to_list", &[a[0].clone()], &[v])?;
                let g = self.c.glue(GlueKind::Show { nested: true }, &a[0]);
                let clo = self.closure_of(g, 2);
                let acc = self.push_static("[", acc);
                let first = self.iconst(1);
                let acc = self.call_std("show_items", &[a[0].clone()], &[items, clo, first, acc])?;
                Ok(self.push_static("]", acc))
            }
            Ty::Con(id, a) if id.ends_with("Map") && a.len() == 2 => {
                let gk = self.c.glue(GlueKind::Show { nested: true }, &a[0]);
                let gv = self.c.glue(GlueKind::Show { nested: true }, &a[1]);
                let (ck, cv) = (self.closure_of(gk, 2), self.closure_of(gv, 2));
                let acc = self.push_static("{", acc);
                let first = self.iconst(1);
                let acc = self.call_std("show_pairs", &[a[0].clone(), a[1].clone()], &[v, ck, cv, first, acc])?;
                Ok(self.push_static("}", acc))
            }
            Ty::Con(id, a) if id.ends_with("Set") && a.len() == 1 => {
                let g = self.c.glue(GlueKind::Show { nested: true }, &a[0]);
                let clo = self.closure_of(g, 2);
                let acc = self.push_static("#{", acc);
                let first = self.iconst(1);
                let acc = self.call_std("show_items", &[a[0].clone()], &[v, clo, first, acc])?;
                Ok(self.push_static("}", acc))
            }
            Ty::Con(..) => self.show_adt(ty, v, acc),
            _ => Err(format!("printing a value of type {ty:?} is not supported by the native backend yet")),
        }
    }

    fn show_adt(&mut self, ty: &Ty, v: ir::Value, acc: ir::Value) -> R<ir::Value> {
        let Ty::Con(id, args) = ty else { unreachable!() };
        if matches!(id.as_str(), "core::Map" | "core::Set") || self.c.type_info(id).is_err() {
            return Err(format!("printing a value of type {ty:?} is not supported by the native backend yet"));
        }
        let ti = self.c.type_info(id)?;
        match &ti.def {
            TDef::Record(fields) => {
                let m: HashMap<String, Ty> = ti.generics.iter().cloned().zip(args.iter().cloned()).collect();
                if cera_sema::ty::short_name(id) == "Error" {
                    let msg = self.load_word(v, 0);
                    return Ok(self.push_chunk(msg, acc));
                }
                let mut acc = self.push_static(&format!("{}{{", cera_syntax::display_name(cera_sema::ty::short_name(id))), acc);
                for (i, (n, t)) in fields.iter().enumerate() {
                    if i > 0 {
                        acc = self.push_static(", ", acc);
                    }
                    acc = self.push_static(&format!("{n}: "), acc);
                    let f = self.load_word(v, i);
                    let ft = norm(&subst_ty(t, &m));
                    acc = self.show_sub(&ft, f, acc)?;
                }
                Ok(self.push_static("}", acc))
            }
            TDef::Variants(vs) => {
                let n = vs.len();
                let merge = self.b.create_block();
                self.b.append_block_param(merge, I64);
                let tag = if n > 1 { self.load_word(v, 0) } else { self.iconst(0) };
                for i in 0..n {
                    let (name, fields, _) = self.variant_fields(id, i, args)?;
                    let body = self.b.create_block();
                    let next = self.b.create_block();
                    if i + 1 == n {
                        self.b.ins().jump(body, &[]);
                    } else {
                        let is = self.b.ins().icmp_imm(IntCC::Equal, tag, i as i64);
                        self.b.ins().brif(is, body, &[], next, &[]);
                    }
                    self.b.switch_to_block(body);
                    let mut a = self.push_static(cera_syntax::display_name(&name), acc);
                    if !fields.is_empty() {
                        a = self.push_static("(", a);
                        for (k, ft) in fields.iter().enumerate() {
                            if k > 0 {
                                a = self.push_static(", ", a);
                            }
                            let f = self.load_word(v, k + 1);
                            a = self.show_sub(ft, f, a)?;
                        }
                        a = self.push_static(")", a);
                    }
                    self.b.ins().jump(merge, &[a.into()]);
                    self.b.switch_to_block(next);
                }
                // `next` of the last variant is unreachable.
                let z = self.iconst(0);
                self.b.ins().jump(merge, &[z.into()]);
                self.b.switch_to_block(merge);
                Ok(self.b.block_params(merge)[0])
            }
            TDef::Opaque => Err(format!("printing a value of type {ty:?} is not supported by the native backend yet")),
        }
    }

    // ----- equality ----------------------------------------------------------------

    /// Emit `a == b` for values of type `ty`, as a 0/1 word.
    pub(crate) fn eq_values(&mut self, ty: &Ty, a: ir::Value, b: ir::Value) -> R<ir::Value> {
        let (a_, b_) = (a, b);
        match ty {
            Ty::Con(n, args) if args.is_empty() && (int_kind(ty).is_some() || matches!(n.as_str(), "bool" | "char")) => {
                let c = self.b.ins().icmp(IntCC::Equal, a, b);
                Ok(self.b.ins().uextend(I64, c))
            }
            Ty::Con(n, args) if args.is_empty() && (n == "f64" || n == "f32") => {
                let x = self.b.ins().bitcast(F64, MemFlags::new(), a);
                let y = self.b.ins().bitcast(F64, MemFlags::new(), b);
                let c = self.b.ins().fcmp(FloatCC::Equal, x, y);
                Ok(self.b.ins().uextend(I64, c))
            }
            Ty::Con(n, args) if args.is_empty() && n == "String" => self.call_std("string_eq", &[], &[a, b]),
            Ty::Tuple(ts) if ts.is_empty() => Ok(self.iconst(1)),
            Ty::Fn(..) => Err("comparing functions is not supported".into()),
            Ty::Con(id, a) if id.ends_with("Map") && a.len() == 2 => {
                let lt = Ty::List(Box::new(Ty::Tuple(a.clone())));
                self.eq_values(&lt, a_, b_)
            }
            Ty::Con(id, a) if id.ends_with("Set") && a.len() == 1 => {
                let lt = Ty::List(Box::new(a[0].clone()));
                self.eq_values(&lt, a_, b_)
            }
            _ => {
                let g = self.c.glue(GlueKind::Eq, ty);
                Ok(rt::call(&mut self.b, &self.c.be, g, 2, &[a, b])[0])
            }
        }
    }

    pub(crate) fn eq_body(&mut self, ty: &Ty, a: ir::Value, b: ir::Value) -> R<ir::Value> {
        let no = self.b.create_block();
        self.b.set_cold_block(no);
        let yes = self.b.create_block();
        match ty {
            Ty::Tuple(ts) => {
                for (i, t) in ts.iter().enumerate() {
                    let (x, y) = (self.load_word(a, i), self.load_word(b, i));
                    let r = self.eq_values(t, x, y)?;
                    self.branch_true(r, no);
                }
            }
            Ty::Ref(inner) => {
                let r = self.eq_values(inner, a, b)?;
                self.branch_true(r, no);
            }
            Ty::List(e) => {
                let lp = self.b.create_block();
                self.b.append_block_param(lp, I64);
                self.b.append_block_param(lp, I64);
                self.b.ins().jump(lp, &[a.into(), b.into()]);
                self.b.switch_to_block(lp);
                let (x, y) = (self.b.block_params(lp)[0], self.b.block_params(lp)[1]);
                let xnil = self.b.ins().icmp_imm(IntCC::Equal, x, 0);
                let ynil = self.b.ins().icmp_imm(IntCC::Equal, y, 0);
                let both = self.b.create_block();
                let either = self.b.create_block();
                let body = self.b.create_block();
                self.b.ins().brif(xnil, both, &[], either, &[]);
                self.b.switch_to_block(both);
                self.b.ins().brif(ynil, yes, &[], no, &[]);
                self.b.switch_to_block(either);
                self.b.ins().brif(ynil, no, &[], body, &[]);
                self.b.switch_to_block(body);
                let (hx, hy) = (self.load_word(x, 0), self.load_word(y, 0));
                let r = self.eq_values(e, hx, hy)?;
                self.branch_true(r, no);
                let (tx, ty_) = (self.load_word(x, 1), self.load_word(y, 1));
                self.b.ins().jump(lp, &[tx.into(), ty_.into()]);
                return Ok(self.finish_eq(yes, no));
            }
            Ty::Con(id, args) => {
                let ti = self.c.type_info(id).map_err(|_| format!("comparing values of type {ty:?} is not supported by the native backend yet"))?;
                match &ti.def {
                    TDef::Record(fields) => {
                        let m: HashMap<String, Ty> = ti.generics.iter().cloned().zip(args.iter().cloned()).collect();
                        for (i, (_, t)) in fields.iter().enumerate() {
                            let ft = norm(&subst_ty(t, &m));
                            let (x, y) = (self.load_word(a, i), self.load_word(b, i));
                            let r = self.eq_values(&ft, x, y)?;
                            self.branch_true(r, no);
                        }
                    }
                    TDef::Variants(vs) => {
                        let n = vs.len();
                        let all_nullary = vs.iter().all(|v| v.fields.is_empty());
                        let (ta, tb) = (self.load_word(a, 0), self.load_word(b, 0));
                        let same = self.b.ins().icmp(IntCC::Equal, ta, tb);
                        let cont = self.b.create_block();
                        self.b.ins().brif(same, cont, &[], no, &[]);
                        self.b.switch_to_block(cont);
                        if !all_nullary {
                            for i in 0..n {
                                let (_, fields, _) = self.variant_fields(id, i, args)?;
                                if fields.is_empty() {
                                    continue;
                                }
                                let body = self.b.create_block();
                                let next = self.b.create_block();
                                let is = self.b.ins().icmp_imm(IntCC::Equal, ta, i as i64);
                                self.b.ins().brif(is, body, &[], next, &[]);
                                self.b.switch_to_block(body);
                                for (k, ft) in fields.iter().enumerate() {
                                    let (x, y) = (self.load_word(a, k + 1), self.load_word(b, k + 1));
                                    let r = self.eq_values(ft, x, y)?;
                                    self.branch_true(r, no);
                                }
                                self.b.ins().jump(yes, &[]);
                                self.b.switch_to_block(next);
                            }
                        }
                    }
                    TDef::Opaque => return Err(format!("comparing values of type {ty:?} is not supported by the native backend yet")),
                }
            }
            _ => return Err(format!("comparing values of type {ty:?} is not supported by the native backend yet")),
        }
        self.b.ins().jump(yes, &[]);
        Ok(self.finish_eq(yes, no))
    }

    fn finish_eq(&mut self, yes: ir::Block, no: ir::Block) -> ir::Value {
        let done = self.b.create_block();
        self.b.append_block_param(done, I64);
        self.b.switch_to_block(yes);
        let one = self.iconst(1);
        self.b.ins().jump(done, &[one.into()]);
        self.b.switch_to_block(no);
        let zero = self.iconst(0);
        self.b.ins().jump(done, &[zero.into()]);
        self.b.switch_to_block(done);
        self.b.block_params(done)[0]
    }

    /// Continue when `r` (a 0/1 word) is true, else jump to `no`.
    fn branch_true(&mut self, r: ir::Value, no: ir::Block) {
        let cont = self.b.create_block();
        self.b.ins().brif(r, cont, &[], no, &[]);
        self.b.switch_to_block(cont);
    }
}

impl<'c, 'a, 'b> FnCx<'c, 'a, 'b> {
    /// `a < b` as a 0/1 word, for types with a built-in or user ordering.
    pub(crate) fn lt_values(&mut self, ty: &Ty, a: ir::Value, b: ir::Value) -> R<ir::Value> {
        if let Some((bits, signed)) = int_kind(ty) {
            let _ = bits;
            let c = self.b.ins().icmp(if signed { IntCC::SignedLessThan } else { IntCC::UnsignedLessThan }, a, b);
            return Ok(self.b.ins().uextend(I64, c));
        }
        match ty {
            Ty::Con(n, args) if args.is_empty() && matches!(n.as_str(), "bool" | "char") => {
                let c = self.b.ins().icmp(IntCC::UnsignedLessThan, a, b);
                Ok(self.b.ins().uextend(I64, c))
            }
            Ty::Con(n, args) if args.is_empty() && (n == "f64" || n == "f32") => {
                let x = self.b.ins().bitcast(F64, MemFlags::new(), a);
                let y = self.b.ins().bitcast(F64, MemFlags::new(), b);
                let c = self.b.ins().fcmp(FloatCC::LessThan, x, y);
                Ok(self.b.ins().uextend(I64, c))
            }
            Ty::Con(n, args) if args.is_empty() && n == "String" => self.call_std("string_less", &[], &[a, b]),
            Ty::Tuple(ts) if ts.is_empty() => Ok(self.iconst(0)),
            Ty::Con(..) => {
                let Some((idx, sub)) = self.c.find_impl("compare", ty) else {
                    return Err(format!("ordering values of type {ty:?} is not supported by the native backend yet"));
                };
                let id = self.c.impl_instance(idx, &sub)?;
                let res = rt::call(&mut self.b, &self.c.be, id, 2, &[a, b])[0];
                let tag = self.load_word(res, 0);
                let c = self.b.ins().icmp_imm(IntCC::Equal, tag, 0);
                Ok(self.b.ins().uextend(I64, c))
            }
            Ty::Tuple(_) | Ty::List(_) => {
                let g = self.c.glue(GlueKind::Lt, ty);
                Ok(rt::call(&mut self.b, &self.c.be, g, 2, &[a, b])[0])
            }
            _ => Err(format!("ordering values of type {ty:?} is not supported by the native backend yet")),
        }
    }

    pub(crate) fn lt_body(&mut self, ty: &Ty, a: ir::Value, b: ir::Value) -> R<ir::Value> {
        let done = self.b.create_block();
        self.b.append_block_param(done, I64);
        match ty {
            Ty::Tuple(ts) => {
                for (i, t) in ts.iter().enumerate() {
                    let (x, y) = (self.load_word(a, i), self.load_word(b, i));
                    let lt = self.lt_values(t, x, y)?;
                    let yes = self.b.create_block();
                    let next = self.b.create_block();
                    self.b.ins().brif(lt, yes, &[], next, &[]);
                    self.b.switch_to_block(yes);
                    let one = self.iconst(1);
                    self.b.ins().jump(done, &[one.into()]);
                    self.b.switch_to_block(next);
                    let gt = self.lt_values(t, y, x)?;
                    let no = self.b.create_block();
                    let cont = self.b.create_block();
                    self.b.ins().brif(gt, no, &[], cont, &[]);
                    self.b.switch_to_block(no);
                    let zero = self.iconst(0);
                    self.b.ins().jump(done, &[zero.into()]);
                    self.b.switch_to_block(cont);
                }
                let zero = self.iconst(0);
                self.b.ins().jump(done, &[zero.into()]);
            }
            Ty::List(e) => {
                let lp = self.b.create_block();
                self.b.append_block_param(lp, I64);
                self.b.append_block_param(lp, I64);
                self.b.ins().jump(lp, &[a.into(), b.into()]);
                self.b.switch_to_block(lp);
                let (x, y) = (self.b.block_params(lp)[0], self.b.block_params(lp)[1]);
                let xnil = self.b.create_block();
                let xcons = self.b.create_block();
                let isnil = self.b.ins().icmp_imm(IntCC::Equal, x, 0);
                self.b.ins().brif(isnil, xnil, &[], xcons, &[]);
                self.b.switch_to_block(xnil);
                let ynn = self.b.ins().icmp_imm(IntCC::NotEqual, y, 0);
                let r = self.b.ins().uextend(I64, ynn);
                self.b.ins().jump(done, &[r.into()]);
                self.b.switch_to_block(xcons);
                let ynil = self.b.create_block();
                let both = self.b.create_block();
                let yn = self.b.ins().icmp_imm(IntCC::Equal, y, 0);
                self.b.ins().brif(yn, ynil, &[], both, &[]);
                self.b.switch_to_block(ynil);
                let zero = self.iconst(0);
                self.b.ins().jump(done, &[zero.into()]);
                self.b.switch_to_block(both);
                let (hx, hy) = (self.load_word(x, 0), self.load_word(y, 0));
                let lt = self.lt_values(e, hx, hy)?;
                let yes = self.b.create_block();
                let next = self.b.create_block();
                self.b.ins().brif(lt, yes, &[], next, &[]);
                self.b.switch_to_block(yes);
                let one = self.iconst(1);
                self.b.ins().jump(done, &[one.into()]);
                self.b.switch_to_block(next);
                let gt = self.lt_values(e, hy, hx)?;
                let no = self.b.create_block();
                let cont = self.b.create_block();
                self.b.ins().brif(gt, no, &[], cont, &[]);
                self.b.switch_to_block(no);
                let zero = self.iconst(0);
                self.b.ins().jump(done, &[zero.into()]);
                self.b.switch_to_block(cont);
                let (tx, ty_) = (self.load_word(x, 1), self.load_word(y, 1));
                self.b.ins().jump(lp, &[tx.into(), ty_.into()]);
            }
            _ => return Err(format!("ordering values of type {ty:?} is not supported by the native backend yet")),
        }
        self.b.switch_to_block(done);
        Ok(self.b.block_params(done)[0])
    }
}
