//! Built-in functions: the runtime side of the standard-library stubs.
//!
//! Every native is written against the `Rt` trait, so the tree-walking
//! interpreter and the bytecode VM share this code.

use crate::rt::{arith, compare, show_value, total_cmp, values_eq, Rt};
use crate::value::*;
use cera_syntax::ast::BinOp;
use cera_syntax::Span;
use std::cmp::Ordering;
use std::io::{BufRead, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::cell::RefCell;
use std::rc::Rc;

/// Every built-in as `"{module}.{name}"` with its arity.
pub const NATIVES: &[(&str, usize)] = &[
    ("core.promote", 1),
    ("core.ref_of", 1),
    ("core.new_in", 2),
    ("core.arena_new", 1),
    ("list.map", 2),
    ("list.filter", 2),
    ("list.fold", 3),
    ("list.sum", 1),
    ("list.length", 1),
    ("list.reverse", 1),
    ("list.append", 2),
    ("list.head", 1),
    ("list.tail", 1),
    ("list.get", 2),
    ("list.last", 1),
    ("list.is_empty", 1),
    ("list.take", 2),
    ("list.drop", 2),
    ("list.concat", 1),
    ("list.flat_map", 2),
    ("list.any", 2),
    ("list.all", 2),
    ("list.find", 2),
    ("list.zip", 2),
    ("list.range", 2),
    ("list.contains", 2),
    ("list.sort", 1),
    ("list.sort_by", 2),
    ("string.length", 1),
    ("string.concat", 1),
    ("string.is_empty", 1),
    ("string.contains", 2),
    ("string.starts_with", 2),
    ("string.ends_with", 2),
    ("string.split", 2),
    ("string.join", 2),
    ("string.trim", 1),
    ("string.to_upper", 1),
    ("string.to_lower", 1),
    ("string.replace", 3),
    ("string.repeat", 2),
    ("string.lines", 1),
    ("string.chars", 1),
    ("string.from_chars", 1),
    ("string.parse_i64", 1),
    ("string.parse_f64", 1),
    ("string.slice", 3),
    ("option.is_some", 1),
    ("option.is_none", 1),
    ("option.unwrap_or", 2),
    ("option.map", 2),
    ("option.and_then", 2),
    ("option.ok_or", 2),
    ("result.is_ok", 1),
    ("result.is_err", 1),
    ("result.unwrap_or", 2),
    ("result.map", 2),
    ("result.map_err", 2),
    ("result.and_then", 2),
    ("result.ok", 1),
    ("bits.band", 2),
    ("bits.bor", 2),
    ("bits.bxor", 2),
    ("bits.bnot", 1),
    ("bits.shl", 2),
    ("bits.shr", 2),
    ("bits.rotl", 2),
    ("bits.rotr", 2),
    ("bits.popcount", 1),
    ("bits.leading_zeros", 1),
    ("bits.trailing_zeros", 1),
    ("bits.wrapping_add", 2),
    ("bits.wrapping_sub", 2),
    ("bits.wrapping_mul", 2),
    ("array.new", 0),
    ("array.filled", 2),
    ("array.from_list", 1),
    ("array.to_list", 1),
    ("array.length", 1),
    ("array.get", 2),
    ("array.set", 3),
    ("array.push", 2),
    ("array.pop", 1),
    ("array.insert", 3),
    ("array.remove", 2),
    ("array.swap", 3),
    ("array.clear", 1),
    ("array.copy", 1),
    ("num.min", 2),
    ("num.max", 2),
    ("num.clamp", 3),
    ("num.abs", 1),
    ("num.pow", 2),
    ("num.sqrt", 1),
    ("num.floor", 1),
    ("num.ceil", 1),
    ("num.round", 1),
    ("num.to_f64", 1),
    ("num.to_i32", 1),
    ("num.to_i64", 1),
    ("num.to_i8", 1),
    ("num.to_i16", 1),
    ("num.to_isize", 1),
    ("num.to_u16", 1),
    ("num.to_u8", 1),
    ("num.to_u32", 1),
    ("num.to_u64", 1),
    ("num.to_usize", 1),
    ("map.new", 0),
    ("map.insert", 3),
    ("map.get", 2),
    ("map.get_or", 3),
    ("map.remove", 2),
    ("map.contains_key", 2),
    ("map.size", 1),
    ("map.is_empty", 1),
    ("map.keys", 1),
    ("map.values", 1),
    ("map.to_list", 1),
    ("map.from_list", 1),
    ("map.merge", 2),
    ("set.new", 0),
    ("set.insert", 2),
    ("set.contains", 2),
    ("set.remove", 2),
    ("set.size", 1),
    ("set.is_empty", 1),
    ("set.to_list", 1),
    ("set.from_list", 1),
    ("set.union", 2),
    ("io.println", 1),
    ("io.print", 1),
    ("io.eprintln", 1),
    ("io.eprint", 1),
    ("io.read_line", 0),
    ("io.read_all", 0),
    ("io.write", 2),
    ("io.writeln", 2),
    ("io.read_line_from", 1),
    ("fs.open_dir", 2),
    ("fs.read_only", 1),
    ("fs.read_to_string", 2),
    ("fs.write_string", 3),
    ("fs.append_string", 3),
    ("fs.exists", 2),
    ("fs.list", 1),
    ("fs.remove", 2),
    ("fs.make_dir", 2),
    ("time.now_ms", 1),
    ("time.sleep_ms", 2),
    ("env.args", 1),
    ("env.var", 2),
    ("process.exit", 1),
    ("mem.addr_of", 1),
    ("mem.buffer", 1),
    ("mem.string_from", 2),
    ("mem.copy_to", 2),
    ("mem.read_u8", 1),
    ("mem.read_u16", 1),
    ("mem.read_u32", 1),
    ("mem.read_u64", 1),
    ("mem.write_u8", 2),
    ("mem.write_u16", 2),
    ("mem.write_u32", 2),
    ("mem.write_u64", 2),
    ("mem.alloc", 1),
    ("mem.free", 2),
    ("mem.realloc", 3),
    ("mem.live", 0),
    ("mem.copy", 3),
    ("mem.fill", 3),
    ("mem.mark", 0),
    ("mem.reset", 1),
    ("sys.syscall0", 1),
    ("sys.syscall1", 2),
    ("sys.syscall2", 3),
    ("sys.syscall3", 4),
    ("sys.syscall4", 5),
    ("sys.syscall5", 6),
    ("sys.syscall6", 7),
];

/// Names of all built-ins, for tests and tools.
pub fn native_names() -> Vec<&'static str> {
    NATIVES.iter().map(|(n, _)| *n).collect()
}

/// The names of the standard-library stubs that are in the prelude.
const PRELUDE_LIST: &[&str] = &[
    "map", "filter", "fold", "sum", "length", "reverse", "append", "head", "tail",
];

fn find_native(id: &str) -> Option<&'static str> {
    NATIVES.iter().find(|(n, _)| *n == id).map(|(n, _)| *n)
}

/// Names available without an import.
pub fn prelude_native(name: &str) -> Option<&'static str> {
    if PRELUDE_LIST.contains(&name) {
        return find_native(&format!("list.{name}"));
    }
    match name {
        "promote" | "ref_of" | "new_in" | "arena_new" => find_native(&format!("core.{name}")),
        _ => None,
    }
}

pub fn module_native(module: &str, member: &str) -> Option<&'static str> {
    let short = module.rsplit('.').next().unwrap_or(module);
    find_native(&format!("{short}.{member}"))
}

// ----- argument helpers ---------------------------------------------------------

fn deref<'x, 'a>(mut v: &'x Value<'a>) -> &'x Value<'a> {
    while let Value::Ref(inner) = v {
        v = inner;
    }
    v
}

fn list<'x, 'a>(args: &'x [Value<'a>], i: usize, name: &str, span: Span) -> Res<'a, &'x [Value<'a>]> {
    match deref(&args[i]).list_items() {
        Some(items) => Ok(items),
        None => rt_err(span, format!("`{name}` needs a list")),
    }
}

fn string<'a>(args: &[Value<'a>], i: usize, name: &str, span: Span) -> Res<'a, Rc<str>> {
    match deref(&args[i]) {
        Value::Str(s) => Ok(s.clone()),
        _ => rt_err(span, format!("`{name}` needs a String")),
    }
}

fn int<'a>(args: &[Value<'a>], i: usize, name: &str, span: Span) -> Res<'a, i128> {
    match deref(&args[i]) {
        Value::Int(x, _) => Ok(*x),
        _ => rt_err(span, format!("`{name}` needs an integer")),
    }
}

fn boolean<'a>(v: Value<'a>, name: &str, span: Span) -> Res<'a, bool> {
    match v {
        Value::Bool(b) => Ok(b),
        _ => rt_err(span, format!("`{name}` needs a function that returns a bool")),
    }
}

fn usize_val<'a>(n: usize) -> Value<'a> {
    Value::Int(n as i128, IntTy::Usize)
}

fn ok_val<'a>(v: Value<'a>) -> Value<'a> {
    Value::ctor("Result", "Ok", vec![v])
}

fn err_val<'a>(msg: impl AsRef<str>) -> Value<'a> {
    let e = Value::Record {
        ty: Rc::from("Error"),
        fields: Rc::new(vec![(Rc::from("message"), Value::str(msg.as_ref()))]),
    };
    Value::ctor("Result", "Err", vec![e])
}

fn opt<'a>(v: Option<Value<'a>>) -> Value<'a> {
    match v {
        Some(x) => Value::some(x),
        None => Value::none(),
    }
}

/// Split an `Option`/`Result` into its constructor name and payload.
fn parts<'x, 'a>(v: &'x Value<'a>, name: &str, span: Span) -> Res<'a, (&'x str, Option<&'x Value<'a>>)> {
    match deref(v) {
        Value::Ctor { tag, fields } => Ok((&*tag.name, fields.first())),
        _ => rt_err(span, format!("`{name}` needs an Option or Result")),
    }
}

fn cap<'x, 'a>(v: &'x Value<'a>, name: &str, span: Span) -> Res<'a, &'x CapVal> {
    match deref(v) {
        Value::Cap(c) => Ok(c),
        _ => rt_err(span, format!("`{name}` needs a capability")),
    }
}

fn merge_sort<'a>(
    v: Vec<Value<'a>>,
    cmp: &mut dyn FnMut(&Value<'a>, &Value<'a>) -> Res<'a, Ordering>,
) -> Res<'a, Vec<Value<'a>>> {
    if v.len() <= 1 {
        return Ok(v);
    }
    let mid = v.len() / 2;
    let right = v[mid..].to_vec();
    let mut left = v;
    left.truncate(mid);
    let l = merge_sort(left, cmp)?;
    let r = merge_sort(right, cmp)?;
    let mut out = Vec::with_capacity(l.len() + r.len());
    let (mut i, mut j) = (0, 0);
    while i < l.len() && j < r.len() {
        if cmp(&r[j], &l[i])? == Ordering::Less {
            out.push(r[j].clone());
            j += 1;
        } else {
            out.push(l[i].clone());
            i += 1;
        }
    }
    out.extend_from_slice(&l[i..]);
    out.extend_from_slice(&r[j..]);
    Ok(out)
}

/// Binary search in sorted data; `Ok(i)` found at `i`, `Err(i)` insert at `i`.
fn bsearch<'x, 'a: 'x, R: Rt<'a> + ?Sized>(
    rt: &R,
    n: usize,
    key: &Value<'a>,
    get: &dyn Fn(usize) -> &'x Value<'a>,
    span: Span,
) -> Res<'a, Result<usize, usize>> {
    let (mut lo, mut hi) = (0, n);
    while lo < hi {
        let mid = (lo + hi) / 2;
        match total_cmp(rt, get(mid), key, span)? {
            Ordering::Less => lo = mid + 1,
            Ordering::Greater => hi = mid,
            Ordering::Equal => return Ok(Ok(mid)),
        }
    }
    Ok(Err(lo))
}

fn map_entries<'x, 'a>(v: &'x Value<'a>, span: Span) -> Res<'a, &'x Rc<Vec<(Value<'a>, Value<'a>)>>> {
    match deref(v) {
        Value::Map(m) => Ok(m),
        _ => rt_err(span, "expected a Map"),
    }
}

fn array_ref<'x, 'a>(v: &'x Value<'a>, span: Span) -> Res<'a, &'x Rc<RefCell<Vec<Value<'a>>>>> {
    match deref(v) {
        Value::Array(a) => Ok(a),
        _ => rt_err(span, "expected an Array"),
    }
}

fn array_index<'a>(a: &[Value<'a>], args: &[Value<'a>], i: usize, short: &str, span: Span) -> Res<'a, usize> {
    let n = int(args, i, short, span)?;
    if n < 0 || n as usize >= a.len() {
        return rt_err(span, format!("index {n} is out of range for an array of length {}", a.len()));
    }
    Ok(n as usize)
}

fn set_items<'x, 'a>(v: &'x Value<'a>, span: Span) -> Res<'a, &'x Rc<Vec<Value<'a>>>> {
    match deref(v) {
        Value::Set(s) => Ok(s),
        _ => rt_err(span, "expected a Set"),
    }
}

fn map_insert<'a, R: Rt<'a> + ?Sized>(
    rt: &R,
    m: &Rc<Vec<(Value<'a>, Value<'a>)>>,
    k: Value<'a>,
    v: Value<'a>,
    span: Span,
) -> Res<'a, Value<'a>> {
    let pos = bsearch(rt, m.len(), &k, &|i| &m[i].0, span)?;
    let mut out = (**m).clone();
    match pos {
        Ok(i) => out[i].1 = v,
        Err(i) => out.insert(i, (k, v)),
    }
    Ok(Value::Map(Rc::new(out)))
}

fn set_insert<'a, R: Rt<'a> + ?Sized>(rt: &R, s: &Rc<Vec<Value<'a>>>, x: Value<'a>, span: Span) -> Res<'a, Value<'a>> {
    let pos = bsearch(rt, s.len(), &x, &|i| &s[i], span)?;
    match pos {
        Ok(_) => Ok(Value::Set(s.clone())),
        Err(i) => {
            let mut out = (**s).clone();
            out.insert(i, x);
            Ok(Value::Set(Rc::new(out)))
        }
    }
}

// ----- the filesystem sandbox ---------------------------------------------------

/// Resolve `name` inside `root`, refusing anything that could leave it.
fn resolve(root: &Path, name: &str) -> Result<PathBuf, String> {
    if name.is_empty() {
        return Err("empty file name".to_string());
    }
    for c in Path::new(name).components() {
        match c {
            Component::Normal(_) | Component::CurDir => {}
            _ => return Err(format!("`{name}` is outside the directory this capability grants")),
        }
    }
    let target = root.join(name);
    // Symlinks must not lead out either: check the nearest existing ancestor.
    let canon_root = root.canonicalize().map_err(|e| e.to_string())?;
    let mut probe = target.clone();
    while !probe.exists() {
        if !probe.pop() {
            break;
        }
    }
    let canon = probe.canonicalize().map_err(|e| e.to_string())?;
    if !canon.starts_with(&canon_root) {
        return Err(format!("`{name}` is outside the directory this capability grants"));
    }
    Ok(target)
}

fn io_err<'a>(name: &str, e: std::io::Error) -> Value<'a> {
    err_val(format!("{name}: {e}"))
}

// ----- the natives ----------------------------------------------------------------

/// Call the built-in `name`.
pub fn call_native<'a, R: Rt<'a> + ?Sized>(rt: &R, name: &str, args: Vec<Value<'a>>, span: Span) -> Res<'a, Value<'a>> {
    let Some((_, arity)) = NATIVES.iter().find(|(n, _)| *n == name) else {
        return rt_err(span, format!("unknown built-in `{name}`"));
    };
    if args.len() != *arity {
        return rt_err(span, format!("`{name}` takes {arity} argument(s), got {}", args.len()));
    }
    let short = name.rsplit('.').next().unwrap_or(name);
    let a = &args[..];
    let host = rt.host();
    Ok(match name {
        // ----- core
        "core.promote" => a[0].clone(),
        "core.ref_of" => Value::Ref(Rc::new(a[0].clone())),
        "core.new_in" => Value::Ref(Rc::new(a[1].clone())),
        "core.arena_new" => Value::Allocator,

        // ----- list
        "list.map" => {
            let mut out = Vec::new();
            for x in list(a, 0, short, span)? {
                out.push(rt.apply1(a[1].clone(), x.clone(), span)?);
            }
            Value::list(out)
        }
        "list.filter" => {
            let mut out = Vec::new();
            for x in list(a, 0, short, span)? {
                if boolean(rt.apply1(a[1].clone(), x.clone(), span)?, short, span)? {
                    out.push(x.clone());
                }
            }
            Value::list(out)
        }
        "list.fold" => {
            let mut acc = a[1].clone();
            for x in list(a, 0, short, span)? {
                acc = rt.apply2(a[2].clone(), acc, x.clone(), span)?;
            }
            acc
        }
        "list.sum" => {
            let items = list(a, 0, short, span)?;
            let Some((first, rest)) = items.split_first() else {
                return rt_err(span, "`sum` of an empty list has no type to start from; use `fold` with a start value");
            };
            let mut acc = first.clone();
            for x in rest {
                acc = arith(rt, BinOp::Add, acc, x.clone(), span)?;
            }
            acc
        }
        "list.length" => usize_val(list(a, 0, short, span)?.len()),
        "list.reverse" => {
            let mut v = list(a, 0, short, span)?.to_vec();
            v.reverse();
            Value::list(v)
        }
        "list.append" => {
            let mut v = list(a, 0, short, span)?.to_vec();
            v.extend_from_slice(list(a, 1, short, span)?);
            Value::list(v)
        }
        "list.head" => opt(list(a, 0, short, span)?.first().cloned()),
        "list.tail" => match deref(&a[0]) {
            Value::List(rc, off) => {
                if *off < rc.len() {
                    Value::some(Value::List(rc.clone(), off + 1))
                } else {
                    Value::none()
                }
            }
            _ => return rt_err(span, "`tail` needs a list"),
        },
        "list.get" => {
            let i = int(a, 1, short, span)?;
            let items = list(a, 0, short, span)?;
            opt(usize::try_from(i).ok().and_then(|i| items.get(i)).cloned())
        }
        "list.last" => opt(list(a, 0, short, span)?.last().cloned()),
        "list.is_empty" => Value::Bool(list(a, 0, short, span)?.is_empty()),
        "list.take" => {
            let n = int(a, 1, short, span)? as usize;
            let items = list(a, 0, short, span)?;
            Value::list(items[..n.min(items.len())].to_vec())
        }
        "list.drop" => match deref(&a[0]) {
            Value::List(rc, off) => {
                let n = int(a, 1, short, span)? as usize;
                Value::List(rc.clone(), (*off).min(rc.len()).saturating_add(n).min(rc.len()))
            }
            _ => return rt_err(span, "`drop` needs a list"),
        },
        "list.concat" => {
            let mut out = Vec::new();
            for inner in list(a, 0, short, span)? {
                match deref(inner).list_items() {
                    Some(items) => out.extend_from_slice(items),
                    None => return rt_err(span, "`concat` needs a list of lists"),
                }
            }
            Value::list(out)
        }
        "list.flat_map" => {
            let mut out = Vec::new();
            for x in list(a, 0, short, span)? {
                let r = rt.apply1(a[1].clone(), x.clone(), span)?;
                match deref(&r).list_items() {
                    Some(items) => out.extend_from_slice(items),
                    None => return rt_err(span, "`flat_map` needs a function that returns a list"),
                }
            }
            Value::list(out)
        }
        "list.any" | "list.all" => {
            let want_any = name == "list.any";
            let mut result = !want_any;
            for x in list(a, 0, short, span)? {
                let b = boolean(rt.apply1(a[1].clone(), x.clone(), span)?, short, span)?;
                if b == want_any {
                    result = want_any;
                    break;
                }
            }
            Value::Bool(result)
        }
        "list.find" => {
            let mut found = None;
            for x in list(a, 0, short, span)? {
                if boolean(rt.apply1(a[1].clone(), x.clone(), span)?, short, span)? {
                    found = Some(x.clone());
                    break;
                }
            }
            opt(found)
        }
        "list.zip" => {
            let (xs, ys) = (list(a, 0, short, span)?, list(a, 1, short, span)?);
            Value::list(
                xs.iter()
                    .zip(ys)
                    .map(|(x, y)| Value::Tuple(Rc::new(vec![x.clone(), y.clone()])))
                    .collect(),
            )
        }
        "list.range" => {
            let (lo, hi) = (int(a, 0, short, span)?, int(a, 1, short, span)?);
            if hi - lo > 100_000_000 {
                return rt_err(span, "`range` is too long");
            }
            Value::list((lo..hi).map(|i| Value::Int(i, IntTy::I64)).collect())
        }
        "list.contains" => {
            for x in list(a, 0, short, span)? {
                if values_eq(rt, x, &a[1], span)? {
                    return Ok(Value::Bool(true));
                }
            }
            Value::Bool(false)
        }
        "list.sort" => {
            let v = list(a, 0, short, span)?.to_vec();
            Value::list(merge_sort(v, &mut |x, y| total_cmp(rt, x, y, span))?)
        }
        "list.sort_by" => {
            let v = list(a, 0, short, span)?.to_vec();
            let f = a[1].clone();
            Value::list(merge_sort(v, &mut |x, y| {
                match rt.apply2(f.clone(), x.clone(), y.clone(), span)? {
                    Value::Ctor { tag, .. } => match &*tag.name {
                        "Less" => Ok(Ordering::Less),
                        "Equal" => Ok(Ordering::Equal),
                        "Greater" => Ok(Ordering::Greater),
                        _ => rt_err(span, "`sort_by` needs a function that returns an Ordering"),
                    },
                    _ => rt_err(span, "`sort_by` needs a function that returns an Ordering"),
                }
            })?)
        }

        // ----- string
        "string.length" => usize_val(string(a, 0, short, span)?.len()),
        "string.concat" => {
            let mut out = String::new();
            for x in list(a, 0, short, span)? {
                match deref(x) {
                    Value::Str(s) => out.push_str(s),
                    _ => return rt_err(span, "`concat` needs a list of Strings"),
                }
            }
            Value::str(&out)
        }
        "string.is_empty" => Value::Bool(string(a, 0, short, span)?.is_empty()),
        "string.contains" => Value::Bool(string(a, 0, short, span)?.contains(&*string(a, 1, short, span)?)),
        "string.starts_with" => Value::Bool(string(a, 0, short, span)?.starts_with(&*string(a, 1, short, span)?)),
        "string.ends_with" => Value::Bool(string(a, 0, short, span)?.ends_with(&*string(a, 1, short, span)?)),
        "string.split" => {
            let (s, sep) = (string(a, 0, short, span)?, string(a, 1, short, span)?);
            if sep.is_empty() {
                return rt_err(span, "`split` needs a non-empty separator; use `chars` to get characters");
            }
            Value::list(s.split(&*sep).map(Value::str).collect())
        }
        "string.join" => {
            let sep = string(a, 1, short, span)?;
            let mut parts = Vec::new();
            for x in list(a, 0, short, span)? {
                match deref(x) {
                    Value::Str(s) => parts.push(s.to_string()),
                    _ => return rt_err(span, "`join` needs a list of Strings"),
                }
            }
            Value::str(&parts.join(&sep))
        }
        "string.trim" => Value::str(string(a, 0, short, span)?.trim()),
        "string.to_upper" => Value::str(&string(a, 0, short, span)?.to_uppercase()),
        "string.to_lower" => Value::str(&string(a, 0, short, span)?.to_lowercase()),
        "string.replace" => {
            let (s, from, to) = (string(a, 0, short, span)?, string(a, 1, short, span)?, string(a, 2, short, span)?);
            if from.is_empty() {
                Value::Str(s)
            } else {
                Value::str(&s.replace(&*from, &to))
            }
        }
        "string.repeat" => {
            let s = string(a, 0, short, span)?;
            let n = int(a, 1, short, span)? as usize;
            if s.len().saturating_mul(n) > 1 << 30 {
                return rt_err(span, "`repeat` would make a string that is too long");
            }
            Value::str(&s.repeat(n))
        }
        "string.lines" => Value::list(string(a, 0, short, span)?.lines().map(Value::str).collect()),
        "string.chars" => Value::list(string(a, 0, short, span)?.chars().map(Value::Char).collect()),
        "string.from_chars" => {
            let mut out = String::new();
            for x in list(a, 0, short, span)? {
                match deref(x) {
                    Value::Char(c) => out.push(*c),
                    _ => return rt_err(span, "`from_chars` needs a list of chars"),
                }
            }
            Value::str(&out)
        }
        "string.parse_i64" => opt(string(a, 0, short, span)?
            .parse::<i64>()
            .ok()
            .map(|n| Value::Int(n as i128, IntTy::I64))),
        "string.parse_f64" => opt(string(a, 0, short, span)?
            .parse::<f64>()
            .ok()
            .map(|n| Value::Float(n, false))),
        "string.slice" => {
            let s = string(a, 0, short, span)?;
            let (start, len) = (int(a, 1, short, span)?, int(a, 2, short, span)?);
            let piece = usize::try_from(start)
                .ok()
                .zip(usize::try_from(len).ok())
                .and_then(|(st, ln)| st.checked_add(ln).map(|end| (st, end)))
                .and_then(|(st, end)| s.get(st..end));
            opt(piece.map(Value::str))
        }

        // ----- option / result
        "option.is_some" => Value::Bool(parts(&a[0], short, span)?.0 == "Some"),
        "option.is_none" => Value::Bool(parts(&a[0], short, span)?.0 == "None"),
        "option.unwrap_or" => match parts(&a[0], short, span)? {
            ("Some", Some(x)) => x.clone(),
            _ => a[1].clone(),
        },
        "option.map" => match parts(&a[0], short, span)? {
            ("Some", Some(x)) => Value::some(rt.apply1(a[1].clone(), x.clone(), span)?),
            _ => Value::none(),
        },
        "option.and_then" => match parts(&a[0], short, span)? {
            ("Some", Some(x)) => rt.apply1(a[1].clone(), x.clone(), span)?,
            _ => Value::none(),
        },
        "option.ok_or" => match parts(&a[0], short, span)? {
            ("Some", Some(x)) => ok_val(x.clone()),
            _ => Value::ctor("Result", "Err", vec![a[1].clone()]),
        },
        "result.is_ok" => Value::Bool(parts(&a[0], short, span)?.0 == "Ok"),
        "result.is_err" => Value::Bool(parts(&a[0], short, span)?.0 == "Err"),
        "result.unwrap_or" => match parts(&a[0], short, span)? {
            ("Ok", Some(x)) => x.clone(),
            _ => a[1].clone(),
        },
        "result.map" => match parts(&a[0], short, span)? {
            ("Ok", Some(x)) => ok_val(rt.apply1(a[1].clone(), x.clone(), span)?),
            _ => a[0].clone(),
        },
        "result.map_err" => match parts(&a[0], short, span)? {
            ("Err", Some(x)) => Value::ctor("Result", "Err", vec![rt.apply1(a[1].clone(), x.clone(), span)?]),
            _ => a[0].clone(),
        },
        "result.and_then" => match parts(&a[0], short, span)? {
            ("Ok", Some(x)) => rt.apply1(a[1].clone(), x.clone(), span)?,
            _ => a[0].clone(),
        },
        "result.ok" => match parts(&a[0], short, span)? {
            ("Ok", Some(x)) => Value::some(x.clone()),
            _ => Value::none(),
        },

        "array.new" => Value::Array(Rc::new(RefCell::new(Vec::new()))),
        "array.filled" => {
            let n = int(a, 0, short, span)?;
            if n < 0 || n > (1 << 32) {
                return rt_err(span, "`filled` count is out of range");
            }
            Value::Array(Rc::new(RefCell::new(vec![a[1].clone(); n as usize])))
        }
        "array.from_list" => Value::Array(Rc::new(RefCell::new(list(a, 0, short, span)?.to_vec()))),
        "array.to_list" => Value::list(array_ref(&a[0], span)?.borrow().clone()),
        "array.length" => usize_val(array_ref(&a[0], span)?.borrow().len()),
        "array.get" => {
            let arr = array_ref(&a[0], span)?.borrow();
            let i = int(a, 1, short, span)?;
            opt(if i < 0 { None } else { arr.get(i as usize).cloned() })
        }
        "array.set" => {
            let mut arr = array_ref(&a[0], span)?.borrow_mut();
            let i = array_index(&arr, a, 1, short, span)?;
            arr[i] = a[2].clone();
            Value::Unit
        }
        "array.push" => {
            array_ref(&a[0], span)?.borrow_mut().push(a[1].clone());
            Value::Unit
        }
        "array.pop" => opt(array_ref(&a[0], span)?.borrow_mut().pop()),
        "array.insert" => {
            let mut arr = array_ref(&a[0], span)?.borrow_mut();
            let n = int(a, 1, short, span)?;
            if n < 0 || n as usize > arr.len() {
                return rt_err(span, format!("index {n} is out of range for inserting into an array of length {}", arr.len()));
            }
            arr.insert(n as usize, a[2].clone());
            Value::Unit
        }
        "array.remove" => {
            let mut arr = array_ref(&a[0], span)?.borrow_mut();
            let i = array_index(&arr, a, 1, short, span)?;
            arr.remove(i)
        }
        "array.swap" => {
            let mut arr = array_ref(&a[0], span)?.borrow_mut();
            let i = array_index(&arr, a, 1, short, span)?;
            let j = array_index(&arr, a, 2, short, span)?;
            arr.swap(i, j);
            Value::Unit
        }
        "array.clear" => {
            array_ref(&a[0], span)?.borrow_mut().clear();
            Value::Unit
        }
        "array.copy" => Value::Array(Rc::new(RefCell::new(array_ref(&a[0], span)?.borrow().clone()))),

        n if n.starts_with("bits.") => {
            let Value::Int(x, ty) = deref(&a[0]) else { return rt_err(span, format!("`{short}` needs an integer")) };
            let (x, ty) = (*x, *ty);
            let (lo, _) = ty.range();
            let width: u32 = match ty {
                IntTy::I8 | IntTy::U8 => 8,
                IntTy::I16 | IntTy::U16 => 16,
                IntTy::I32 | IntTy::U32 => 32,
                _ => 64,
            };
            let mask: u128 = if width == 64 { u64::MAX as u128 } else { (1u128 << width) - 1 };
            let ub = |v: i128| (v as u128) & mask;
            let norm = |b: u128| -> Value<'a> {
                let b = b & mask;
                let v = if lo < 0 && (b >> (width - 1)) & 1 == 1 { b as i128 - (1i128 << width) } else { b as i128 };
                Value::Int(v, ty)
            };
            let other = |i: usize| -> Res<'a, i128> {
                match deref(&a[i]) {
                    Value::Int(y, t2) if *t2 == ty || i == 1 && matches!(short, "shl" | "shr" | "rotl" | "rotr") => Ok(*y),
                    _ => rt_err(span, format!("`{short}` needs integers of the same type")),
                }
            };
            let u32v = |v: u32| Value::Int(v as i128, IntTy::U32);
            match short {
                "band" => norm(ub(x) & ub(other(1)?)),
                "bor" => norm(ub(x) | ub(other(1)?)),
                "bxor" => norm(ub(x) ^ ub(other(1)?)),
                "bnot" => norm(!ub(x)),
                "shl" | "shr" | "rotl" | "rotr" => {
                    let k = other(1)?;
                    if short.starts_with("sh") && (k < 0 || k >= width as i128) {
                        return rt_err(span, format!("shift amount {k} is not less than the width of `{}`", ty.name()));
                    }
                    let k = (k.rem_euclid(width as i128)) as u32;
                    match short {
                        "shl" => norm(ub(x) << k),
                        "shr" => if lo < 0 { norm((x >> k) as u128) } else { norm(ub(x) >> k) },
                        "rotl" => norm(if k == 0 { ub(x) } else { (ub(x) << k) | (ub(x) >> (width - k)) }),
                        _ => norm(if k == 0 { ub(x) } else { (ub(x) >> k) | (ub(x) << (width - k)) }),
                    }
                }
                "popcount" => u32v(ub(x).count_ones()),
                "leading_zeros" => u32v(ub(x).leading_zeros() - (128 - width)),
                "trailing_zeros" => u32v(if ub(x) == 0 { width } else { ub(x).trailing_zeros() }),
                "wrapping_add" => norm(ub(x).wrapping_add(ub(other(1)?))),
                "wrapping_sub" => norm(ub(x).wrapping_sub(ub(other(1)?))),
                "wrapping_mul" => norm(ub(x).wrapping_mul(ub(other(1)?))),
                _ => return rt_err(span, format!("built-in `{name}` is not implemented")),
            }
        }

        // ----- num
        "num.min" | "num.max" => {
            let o = total_cmp(rt, &a[0], &a[1], span)?;
            let first = if name == "num.min" { o != Ordering::Greater } else { o != Ordering::Less };
            if first { a[0].clone() } else { a[1].clone() }
        }
        "num.clamp" => {
            if total_cmp(rt, &a[1], &a[2], span)? == Ordering::Greater {
                return rt_err(span, "`clamp` needs low <= high");
            }
            if total_cmp(rt, &a[0], &a[1], span)? == Ordering::Less {
                a[1].clone()
            } else if total_cmp(rt, &a[0], &a[2], span)? == Ordering::Greater {
                a[2].clone()
            } else {
                a[0].clone()
            }
        }
        "num.abs" => match deref(&a[0]) {
            Value::Int(x, t) => {
                if t.fits(x.abs()) {
                    Value::Int(x.abs(), *t)
                } else {
                    return rt_err(span, "integer overflow in `abs`");
                }
            }
            Value::Float(x, f) => Value::Float(x.abs(), *f),
            _ => return rt_err(span, "`abs` needs a number"),
        },
        "num.pow" => {
            let e = int(a, 1, short, span)?;
            match deref(&a[0]) {
                Value::Int(_, t) => {
                    let mut acc = Value::Int(1, *t);
                    for _ in 0..e {
                        acc = arith(rt, BinOp::Mul, acc, a[0].clone(), span)?;
                    }
                    acc
                }
                Value::Float(x, f) => {
                    let r = x.powi(e as i32);
                    Value::Float(if *f { r as f32 as f64 } else { r }, *f)
                }
                _ => return rt_err(span, "`pow` needs a number"),
            }
        }
        "num.sqrt" | "num.floor" | "num.ceil" | "num.round" => match deref(&a[0]) {
            Value::Float(x, f) => {
                let r = match name {
                    "num.sqrt" => x.sqrt(),
                    "num.floor" => x.floor(),
                    "num.ceil" => x.ceil(),
                    _ => x.round(),
                };
                Value::Float(r, *f)
            }
            _ => return rt_err(span, format!("`{short}` needs a float")),
        },
        "num.to_f64" => match deref(&a[0]) {
            Value::Int(x, _) => Value::Float(*x as f64, false),
            Value::Float(x, _) => Value::Float(*x, false),
            _ => return rt_err(span, "`to_f64` needs a number"),
        },
        "num.to_i8" | "num.to_i16" | "num.to_i32" | "num.to_i64" | "num.to_isize" | "num.to_u8" | "num.to_u16" | "num.to_u32" | "num.to_u64" | "num.to_usize" => {
            let ty = IntTy::from_name(&short[3..]).expect("conversion target");
            let v = match deref(&a[0]) {
                Value::Int(x, _) => Some(*x),
                Value::Float(x, _) if x.is_finite() && x.trunc().abs() < 1e30 => Some(x.trunc() as i128),
                Value::Float(..) => None,
                _ => return rt_err(span, format!("`{short}` needs a number")),
            };
            opt(v.filter(|v| ty.fits(*v)).map(|v| Value::Int(v, ty)))
        }

        // ----- map
        "map.new" => Value::Map(Rc::new(Vec::new())),
        "map.insert" => map_insert(rt, map_entries(&a[0], span)?, a[1].clone(), a[2].clone(), span)?,
        "map.get" | "map.get_or" | "map.contains_key" | "map.remove" => {
            let m = map_entries(&a[0], span)?;
            let pos = bsearch(rt, m.len(), &a[1], &|i| &m[i].0, span)?;
            match name {
                "map.get" => opt(pos.ok().map(|i| m[i].1.clone())),
                "map.get_or" => pos.map(|i| m[i].1.clone()).unwrap_or_else(|_| a[2].clone()),
                "map.contains_key" => Value::Bool(pos.is_ok()),
                _ => match pos {
                    Ok(i) => {
                        let mut out = (**m).clone();
                        out.remove(i);
                        Value::Map(Rc::new(out))
                    }
                    Err(_) => Value::Map(m.clone()),
                },
            }
        }
        "map.size" => usize_val(map_entries(&a[0], span)?.len()),
        "map.is_empty" => Value::Bool(map_entries(&a[0], span)?.is_empty()),
        "map.keys" => Value::list(map_entries(&a[0], span)?.iter().map(|(k, _)| k.clone()).collect()),
        "map.values" => Value::list(map_entries(&a[0], span)?.iter().map(|(_, v)| v.clone()).collect()),
        "map.to_list" => Value::list(
            map_entries(&a[0], span)?
                .iter()
                .map(|(k, v)| Value::Tuple(Rc::new(vec![k.clone(), v.clone()])))
                .collect(),
        ),
        "map.from_list" => {
            let mut m = Rc::new(Vec::new());
            for pair in list(a, 0, short, span)? {
                match deref(pair) {
                    Value::Tuple(kv) if kv.len() == 2 => {
                        if let Value::Map(next) = map_insert(rt, &m, kv[0].clone(), kv[1].clone(), span)? {
                            m = next;
                        }
                    }
                    _ => return rt_err(span, "`from_list` needs a list of pairs"),
                }
            }
            Value::Map(m)
        }
        "map.merge" => {
            let mut m = map_entries(&a[0], span)?.clone();
            for (k, v) in map_entries(&a[1], span)?.iter() {
                if let Value::Map(next) = map_insert(rt, &m, k.clone(), v.clone(), span)? {
                    m = next;
                }
            }
            Value::Map(m)
        }

        // ----- set
        "set.new" => Value::Set(Rc::new(Vec::new())),
        "set.insert" => set_insert(rt, set_items(&a[0], span)?, a[1].clone(), span)?,
        "set.contains" => {
            let s = set_items(&a[0], span)?;
            Value::Bool(bsearch(rt, s.len(), &a[1], &|i| &s[i], span)?.is_ok())
        }
        "set.remove" => {
            let s = set_items(&a[0], span)?;
            match bsearch(rt, s.len(), &a[1], &|i| &s[i], span)? {
                Ok(i) => {
                    let mut out = (**s).clone();
                    out.remove(i);
                    Value::Set(Rc::new(out))
                }
                Err(_) => Value::Set(s.clone()),
            }
        }
        "set.size" => usize_val(set_items(&a[0], span)?.len()),
        "set.is_empty" => Value::Bool(set_items(&a[0], span)?.is_empty()),
        "set.to_list" => Value::list((**set_items(&a[0], span)?).clone()),
        "set.from_list" => {
            let mut s = Rc::new(Vec::new());
            for x in list(a, 0, short, span)? {
                if let Value::Set(next) = set_insert(rt, &s, x.clone(), span)? {
                    s = next;
                }
            }
            Value::Set(s)
        }
        "set.union" => {
            let mut s = set_items(&a[0], span)?.clone();
            for x in set_items(&a[1], span)?.iter() {
                if let Value::Set(next) = set_insert(rt, &s, x.clone(), span)? {
                    s = next;
                }
            }
            Value::Set(s)
        }

        // ----- io
        "io.println" | "io.print" | "io.eprintln" | "io.eprint" => {
            let mut s = show_value(rt, &a[0], false)?;
            if name.ends_with("ln") {
                s.push('\n');
            }
            let to_err = short.starts_with('e');
            let r = if to_err {
                host.err.borrow_mut().write_all(s.as_bytes())
            } else {
                host.out.borrow_mut().write_all(s.as_bytes())
            };
            if r.is_err() {
                // The reader went away (a closed pipe); stop quietly.
                return Err(Ctl::Exit(0));
            }
            Value::Unit
        }
        "io.write" | "io.writeln" => {
            if !matches!(cap(&a[0], short, span)?, CapVal::Console) {
                return rt_err(span, format!("`{short}` needs a Console capability"));
            }
            let mut s = string(a, 1, short, span)?.to_string();
            if name == "io.writeln" {
                s.push('\n');
            }
            if host.out.borrow_mut().write_all(s.as_bytes()).is_err() {
                return Err(Ctl::Exit(0));
            }
            Value::Unit
        }
        "io.read_line" | "io.read_line_from" => {
            if name == "io.read_line_from" && !matches!(cap(&a[0], short, span)?, CapVal::Console) {
                return rt_err(span, "`read_line_from` needs a Console capability");
            }
            let _ = host.out.borrow_mut().flush();
            let mut line = String::new();
            match host.input.borrow_mut().read_line(&mut line) {
                Ok(0) => Value::none(),
                Ok(_) => {
                    if line.ends_with('\n') {
                        line.pop();
                        if line.ends_with('\r') {
                            line.pop();
                        }
                    }
                    Value::some(Value::str(&line))
                }
                Err(e) => return rt_err(span, format!("cannot read input: {e}")),
            }
        }
        "io.read_all" => {
            let _ = host.out.borrow_mut().flush();
            let mut text = String::new();
            if let Err(e) = host.input.borrow_mut().read_to_string(&mut text) {
                return rt_err(span, format!("cannot read input: {e}"));
            }
            Value::str(&text)
        }

        // ----- fs
        "fs.read_only" => match cap(&a[0], short, span)? {
            CapVal::Dir { root, .. } => Value::Cap(Rc::new(CapVal::Dir {
                root: root.clone(),
                read_only: true,
            })),
            _ => return rt_err(span, "`read_only` needs a Dir capability"),
        },
        "fs.open_dir" | "fs.read_to_string" | "fs.write_string" | "fs.append_string" | "fs.exists"
        | "fs.list" | "fs.remove" | "fs.make_dir" => {
            let CapVal::Dir { root, read_only } = cap(&a[0], short, span)? else {
                return rt_err(span, format!("`{short}` needs a Dir capability"));
            };
            let writes = matches!(short, "write_string" | "append_string" | "remove" | "make_dir");
            if writes && *read_only {
                return Ok(err_val("this directory is read-only"));
            }
            if short == "list" {
                return Ok(match std::fs::read_dir(root) {
                    Ok(rd) => {
                        let mut names: Vec<String> = rd
                            .filter_map(|e| e.ok())
                            .map(|e| e.file_name().to_string_lossy().into_owned())
                            .collect();
                        names.sort();
                        ok_val(Value::list(names.iter().map(|n| Value::str(n)).collect()))
                    }
                    Err(e) => io_err(".", e),
                });
            }
            let fname = string(a, 1, short, span)?;
            let path = match resolve(root, &fname) {
                Ok(p) => p,
                Err(msg) => {
                    return Ok(if short == "exists" { Value::Bool(false) } else { err_val(msg) });
                }
            };
            match short {
                "open_dir" => {
                    if path.is_dir() {
                        ok_val(Value::Cap(Rc::new(CapVal::Dir {
                            root: path,
                            read_only: *read_only,
                        })))
                    } else {
                        err_val(format!("{fname}: not a directory"))
                    }
                }
                "read_to_string" => match std::fs::read_to_string(&path) {
                    Ok(s) => ok_val(Value::str(&s)),
                    Err(e) => io_err(&fname, e),
                },
                "write_string" => {
                    let data = string(a, 2, short, span)?;
                    match std::fs::write(&path, data.as_bytes()) {
                        Ok(()) => ok_val(Value::Unit),
                        Err(e) => io_err(&fname, e),
                    }
                }
                "append_string" => {
                    let data = string(a, 2, short, span)?;
                    let r = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&path)
                        .and_then(|mut f| f.write_all(data.as_bytes()));
                    match r {
                        Ok(()) => ok_val(Value::Unit),
                        Err(e) => io_err(&fname, e),
                    }
                }
                "exists" => Value::Bool(path.exists()),
                "remove" => {
                    let r = if path.is_dir() {
                        std::fs::remove_dir(&path)
                    } else {
                        std::fs::remove_file(&path)
                    };
                    match r {
                        Ok(()) => ok_val(Value::Unit),
                        Err(e) => io_err(&fname, e),
                    }
                }
                _ => match std::fs::create_dir(&path) {
                    Ok(()) => ok_val(Value::Unit),
                    Err(e) => io_err(&fname, e),
                },
            }
        }

        // ----- time, env, process
        "time.now_ms" => {
            if !matches!(cap(&a[0], short, span)?, CapVal::Clock) {
                return rt_err(span, "`now_ms` needs a Clock capability");
            }
            let ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i128)
                .unwrap_or(0);
            Value::Int(ms, IntTy::I64)
        }
        "time.sleep_ms" => {
            if !matches!(cap(&a[0], short, span)?, CapVal::Clock) {
                return rt_err(span, "`sleep_ms` needs a Clock capability");
            }
            let ms = int(a, 1, short, span)?;
            let _ = host.out.borrow_mut().flush();
            std::thread::sleep(std::time::Duration::from_millis(ms.max(0) as u64));
            Value::Unit
        }
        "env.args" => {
            if !matches!(cap(&a[0], short, span)?, CapVal::Env) {
                return rt_err(span, "`args` needs an Env capability");
            }
            Value::list(host.args.iter().map(|s| Value::str(s)).collect())
        }
        "env.var" => {
            if !matches!(cap(&a[0], short, span)?, CapVal::Env) {
                return rt_err(span, "`var` needs an Env capability");
            }
            opt(std::env::var(&*string(a, 1, short, span)?).ok().map(|v| Value::str(&v)))
        }
        "process.exit" => return Err(Ctl::Exit(int(a, 0, short, span)? as i32)),
        n if n.starts_with("mem.") || n.starts_with("sys.") => {
            return rt_err(span, format!("`{n}` needs the native backend (run with `--native`, or build an executable)"));
        }

        _ => return rt_err(span, format!("built-in `{name}` is not implemented")),
    })
}

/// Trait methods on built-in types.
pub fn native_method<'a, R: Rt<'a> + ?Sized>(rt: &R, method: &str, args: Vec<Value<'a>>, span: Span) -> Res<'a, Value<'a>> {
    match method {
        "show" => Ok(Value::str(&show_value(rt, &args[0], false)?)),
        "compare" if args.len() == 2 => match compare(rt, &args[0], &args[1], span)? {
            Some(o) => Ok(Value::ordering(o)),
            None => rt_err(span, "cannot order these values (NaN)"),
        },
        "add" | "sub" | "mul" | "div" | "rem" if args.len() == 2 => {
            let op = match method {
                "add" => BinOp::Add,
                "sub" => BinOp::Sub,
                "mul" => BinOp::Mul,
                "div" => BinOp::Div,
                _ => BinOp::Rem,
            };
            match (&args[0], &args[1]) {
                (Value::Int(..), Value::Int(..)) | (Value::Float(..), Value::Float(..)) => {
                    arith(rt, op, args[0].clone(), args[1].clone(), span)
                }
                _ => rt_err(span, format!("`{method}` is not implemented for this type")),
            }
        }
        "neg" => match &args[0] {
            Value::Int(x, t) if t.fits(-x) => Ok(Value::Int(-x, *t)),
            Value::Int(..) => rt_err(span, "integer overflow"),
            Value::Float(x, f) => Ok(Value::Float(-x, *f)),
            _ => rt_err(span, "`neg` is not implemented for this type"),
        },
        _ => rt_err(span, format!("no implementation of `{method}` for this type")),
    }
}
