//! `repr type` records: C-style memory layout and the accessor functions generated for them.
//!
//! `Header :: repr type { magic: u32, len: u16, flags: u8 }` is an ordinary record plus
//! these functions (named after the type in snake case, here `header`):
//!
//! - `header_size()`, `header_align()`: bytes and alignment of the layout.
//! - `header_<field>_offset()`: byte offset of a field.
//! - `unsafe header_read(addr)`: load a record from memory.
//! - `unsafe header_write(addr, value)`: store a record to memory.
//!
//! Fields are laid out in order, each aligned to its own size, and the total size is rounded
//! up to the largest alignment (the C rule). Field types: `i8` to `i64`, `u8` to `u64`,
//! `isize`, `usize` (8 bytes, also used for raw pointers), `bool` (1 byte), another `repr`
//! record declared earlier in the same file, and `[T; N]` for `N` inline elements (read and
//! written as a list of `N` elements).

use crate::ast::*;
use std::collections::HashMap;

pub type Layouts = HashMap<String, (u64, u64)>;

pub fn snake(name: &str) -> String {
    let mut out = String::new();
    let chars: Vec<char> = name.chars().collect();
    for (i, c) in chars.iter().enumerate() {
        if c.is_uppercase() {
            let prev_lower = i > 0 && (chars[i - 1].is_lowercase() || chars[i - 1].is_ascii_digit());
            let next_lower = i + 1 < chars.len() && chars[i + 1].is_lowercase();
            if i > 0 && (prev_lower || (chars[i - 1].is_uppercase() && next_lower)) {
                out.push('_');
            }
            out.extend(c.to_lowercase());
        } else {
            out.push(*c);
        }
    }
    out
}

enum Scalar {
    Int { bytes: u64, signed: bool },
    Bool,
    Nested(String, u64, u64),
}

fn scalar(ty: &Type, layouts: &Layouts) -> Result<(Scalar, u64, u64), String> {
    let TypeKind::Path { path, args } = &ty.kind else {
        return Err("a repr field must be an integer, `bool`, a repr record, or `[T; N]`".into());
    };
    if path.len() != 1 || !args.is_empty() {
        return Err("a repr field must be an integer, `bool`, a repr record, or `[T; N]`".into());
    }
    let n = path[0].name.as_str();
    let int = |bytes: u64, signed: bool| Ok((Scalar::Int { bytes, signed }, bytes, bytes));
    match n {
        "u8" => int(1, false),
        "u16" => int(2, false),
        "u32" => int(4, false),
        "u64" | "usize" => int(8, false),
        "i8" => int(1, true),
        "i16" => int(2, true),
        "i32" => int(4, true),
        "i64" | "isize" => int(8, true),
        "bool" => Ok((Scalar::Bool, 1, 1)),
        _ => match layouts.get(n) {
            Some(&(size, align)) => Ok((Scalar::Nested(n.to_string(), size, align), size, align)),
            None => Err(format!("`{n}` cannot be a repr field: use an integer, `bool`, `usize`, or a repr record declared earlier")),
        },
    }
}

fn is_usize(ty: &Type) -> Option<&'static str> {
    if let TypeKind::Path { path, .. } = &ty.kind {
        if path.len() == 1 {
            return match path[0].name.as_str() {
                "usize" => Some("usize"),
                "isize" => Some("isize"),
                _ => None,
            };
        }
    }
    None
}

fn elem_ty<'a>(f: &'a Field) -> &'a Type {
    match (&f.array, &f.ty.kind) {
        (Some(_), TypeKind::List(inner)) => inner,
        _ => &f.ty,
    }
}

fn read_expr(ty: &Type, sc: &Scalar, addr: &str) -> String {
    match sc {
        Scalar::Bool => format!("(repr_mem.read_u8({addr}) != 0)"),
        Scalar::Nested(n, ..) => format!("{}_read({addr})", snake(n)),
        Scalar::Int { bytes, signed } => {
            let raw = format!("repr_mem.read_u{}({addr})", bytes * 8);
            if is_usize(ty) == Some("usize") {
                return format!("Option.unwrap_or(Num.to_usize({raw}), 0)");
            }
            if !*signed {
                return raw;
            }
            let bits = bytes * 8;
            let ty_name = match &ty.kind {
                TypeKind::Path { path, .. } => path[0].name.clone(),
                _ => unreachable!(),
            };
            let conv = if ty_name == "isize" { "isize".to_string() } else { format!("i{bits}") };
            if bits == 64 {
                // Split around the sign bit so no intermediate value overflows.
                let low = format!("Option.unwrap_or(Num.to_i64(repr_bits.band({raw}, 9223372036854775807)), 0)");
                let high = format!("repr_bits.shr({raw}, 63)");
                let adj = format!("(if {high} == 1 do -9223372036854775807 - 1 else 0 end)");
                if conv == "isize" {
                    format!("Option.unwrap_or(Num.to_isize({low} + {adj}), 0)")
                } else {
                    format!("({low} + {adj})")
                }
            } else {
                let full: u64 = 1 << bits;
                let half: u64 = 1 << (bits - 1);
                let wide = format!("Option.unwrap_or(Num.to_i64({raw}), 0)");
                format!(
                    "Option.unwrap_or(Num.to_{conv}((if {wide} >= {half} do {wide} - {full} else {wide} end)), 0)"
                )
            }
        }
    }
}

fn write_stmt(ty: &Type, sc: &Scalar, addr: &str, value: &str) -> String {
    match sc {
        Scalar::Bool => format!("repr_mem.write_u8({addr}, if {value} do 1 else 0 end)"),
        Scalar::Nested(n, ..) => format!("{}_write({addr}, {value})", snake(n)),
        Scalar::Int { bytes, signed } => {
            let bits = bytes * 8;
            let w = format!("repr_mem.write_u{bits}");
            if is_usize(ty) == Some("usize") {
                return format!("{w}({addr}, Option.unwrap_or(Num.to_u64({value}), 0))");
            }
            if !*signed {
                return format!("{w}({addr}, {value})");
            }
            if bits == 64 {
                // Negative values: two's complement is 2^64 + v, built from two halves.
                let wide = format!("Option.unwrap_or(Num.to_i64({value}), 0)");
                format!(
                    "{w}({addr}, if {wide} >= 0 do Option.unwrap_or(Num.to_u64({wide}), 0) else Option.unwrap_or(Num.to_u64({wide} + 9223372036854775807 + 1), 0) + repr_bits.shl(Option.unwrap_or(Num.to_u64(1), 0), 63) end)"
                )
            } else {
                let full: u64 = 1 << bits;
                let wide = format!("Option.unwrap_or(Num.to_i64({value}), 0)");
                let u = format!("u{bits}");
                format!(
                    "{w}({addr}, Option.unwrap_or(Num.to_{u}((if {wide} < 0 do {wide} + {full} else {wide} end)), 0))"
                )
            }
        }
    }
}

/// The generated declarations for a `repr type` record, as source text; also records the
/// layout so later `repr` records can nest this one.
pub fn generate(d: &Decl, fields: &[Field], layouts: &mut Layouts) -> Result<String, String> {
    if !d.generics.is_empty() {
        return Err("a repr type cannot be generic".into());
    }
    let name = &d.name.name;
    let sn = snake(name);
    let pub_ = if d.public { "pub " } else { "" };
    let mut offset = 0u64;
    let mut max_align = 1u64;
    let mut offsets = Vec::new();
    let mut reads = Vec::new();
    let mut writes = Vec::new();
    for f in fields {
        let elem = elem_ty(f);
        let (sc, size, align) = scalar(elem, layouts)?;
        offset = offset.div_ceil(align) * align;
        offsets.push(offset);
        max_align = max_align.max(align);
        let fname = &f.name.name;
        match f.array {
            None => {
                reads.push(format!("{fname}: {}", read_expr(elem, &sc, &format!("addr + {offset}"))));
                writes.push(write_stmt(elem, &sc, &format!("addr + {offset}"), &format!("value.{fname}")));
                offset += size;
            }
            Some(n) => {
                if n == 0 || n > 256 {
                    return Err("a repr array needs between 1 and 256 elements".into());
                }
                let items: Vec<String> =
                    (0..n).map(|i| read_expr(elem, &sc, &format!("addr + {}", offset + i * size))).collect();
                reads.push(format!("{fname}: [{}]", items.join(", ")));
                // Elements missing from a short list are written as zero.
                let zero = match sc {
                    Scalar::Bool => "false".to_string(),
                    Scalar::Nested(..) => return Err("arrays of repr records are not supported".into()),
                    Scalar::Int { .. } => "0".to_string(),
                };
                for i in 0..n {
                    let v = format!("Option.unwrap_or(List.get(value.{fname}, {i}), {zero})");
                    writes.push(write_stmt(elem, &sc, &format!("addr + {}", offset + i * size), &v));
                }
                offset += size * n;
            }
        }
    }
    let size = offset.div_ceil(max_align) * max_align;
    layouts.insert(name.clone(), (size.max(0), max_align));
    let mut out = String::new();
    out.push_str(&format!("{pub_}{sn}_size :: fn() -> usize do {size} end\n"));
    out.push_str(&format!("{pub_}{sn}_align :: fn() -> usize do {max_align} end\n"));
    for (f, o) in fields.iter().zip(&offsets) {
        out.push_str(&format!("{pub_}{sn}_{}_offset :: fn() -> usize do {o} end\n", f.name.name));
    }
    out.push_str(&format!(
        "{pub_}{sn}_read :: unsafe fn(addr: usize) -> {name} do\n  {name}{{{}}}\nend\n",
        reads.join(", ")
    ));
    if writes.is_empty() {
        writes.push("()".into());
    }
    out.push_str(&format!(
        "{pub_}{sn}_write :: unsafe fn(addr: usize, value: {name}) do\n  {}\nend\n",
        writes.join("\n  ")
    ));
    Ok(out)
}
