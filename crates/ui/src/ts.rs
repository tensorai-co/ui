//! TypeScript declarations for IPC types and calls (`ipc_types!`, `ui-pack gen-ts`).

use std::collections::BTreeMap;
use std::marker::PhantomData;
use std::path::PathBuf;

use crate::ipc::{Bytes, Value};

/// A Rust type's TypeScript spelling, for generated `.d.ts` files.
///
/// Implemented for primitives, `String`, `Bytes`, `Option`, `Vec`, tuples,
/// `PathBuf`, `Value`, and every struct declared with [`ipc_struct!`](crate::ipc_struct).
pub trait TsType {
    /// The type as written in TypeScript, e.g. `string`, `number[]`, `ExportRequest`.
    fn ts_type() -> String;
    /// Adds the declarations this type needs (its own `interface`, nested ones) to `out`.
    fn ts_declarations(_out: &mut BTreeMap<String, String>) {}
}

macro_rules! ts_prim {
    ($ts:literal: $($t:ty),*) => { $(impl TsType for $t { fn ts_type() -> String { $ts.into() } })* };
}
ts_prim!("number": u8, u16, u32, u64, usize, i8, i16, i32, i64, isize, f32, f64);
ts_prim!("boolean": bool);
ts_prim!("string": String, &str, PathBuf);
ts_prim!("null": ());
ts_prim!("Uint8Array": Bytes);
ts_prim!("unknown": Value);

impl<T: TsType> TsType for Option<T> {
    fn ts_type() -> String {
        format!("{} | null", T::ts_type())
    }
    fn ts_declarations(out: &mut BTreeMap<String, String>) {
        T::ts_declarations(out);
    }
}

impl<T: TsType> TsType for Vec<T> {
    fn ts_type() -> String {
        let inner = T::ts_type();
        if inner.contains(' ') {
            format!("({inner})[]")
        } else {
            format!("{inner}[]")
        }
    }
    fn ts_declarations(out: &mut BTreeMap<String, String>) {
        T::ts_declarations(out);
    }
}

macro_rules! ts_tuple {
    ($($t:ident),+) => {
        impl<$($t: TsType),+> TsType for ($($t,)+) {
            fn ts_type() -> String {
                let parts: Vec<String> = vec![$($t::ts_type()),+];
                format!("[{}]", parts.join(", "))
            }
            fn ts_declarations(out: &mut BTreeMap<String, String>) {
                $($t::ts_declarations(out);)+
            }
        }
    };
}
ts_tuple!(A);
ts_tuple!(A, B);
ts_tuple!(A, B, C);
ts_tuple!(A, B, C, D);

// Probe used by the macros: picks `TsType` when a type has it, else `unknown`
// (method resolution prefers the by-reference impl), so a struct field of a
// type without `TsType` still compiles.
#[doc(hidden)]
pub struct __TsProbe<T: ?Sized>(PhantomData<T>);

impl<T: ?Sized> __TsProbe<T> {
    pub const fn new() -> Self {
        Self(PhantomData)
    }
}

impl<T: ?Sized> Default for __TsProbe<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[doc(hidden)]
pub trait __TsViaType {
    fn __ts_type(&self) -> String;
    fn __ts_declarations(&self, out: &mut BTreeMap<String, String>);
}

impl<T: TsType> __TsViaType for __TsProbe<T> {
    fn __ts_type(&self) -> String {
        T::ts_type()
    }
    fn __ts_declarations(&self, out: &mut BTreeMap<String, String>) {
        T::ts_declarations(out);
    }
}

#[doc(hidden)]
pub trait __TsFallback {
    fn __ts_type(&self) -> String;
    fn __ts_declarations(&self, out: &mut BTreeMap<String, String>);
}

impl<T: ?Sized> __TsFallback for &__TsProbe<T> {
    fn __ts_type(&self) -> String {
        "unknown".into()
    }
    fn __ts_declarations(&self, _out: &mut BTreeMap<String, String>) {}
}

#[doc(hidden)]
pub fn __is_optional(ts: &str) -> bool {
    ts.ends_with(" | null")
}

/// Writes generated TypeScript when `ui-pack gen-ts` asks for it.
///
/// Call it first thing in `main`: when `UI_GEN_TS=<path>` is set, it writes
/// `typescript()` there and exits; otherwise it does nothing.
///
/// ```ignore
/// fn main() -> ui::Result<()> {
///     ui::write_typescript_if_requested(ipc_typescript);
///     ui::run(|app| { … })
/// }
/// ```
pub fn write_typescript_if_requested(typescript: impl FnOnce() -> String) {
    let Some(path) = std::env::var_os("UI_GEN_TS") else {
        return;
    };
    let path = PathBuf::from(path);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    match std::fs::write(&path, typescript()) {
        Ok(()) => std::process::exit(0),
        Err(e) => {
            eprintln!("ui: writing {}: {e}", path.display());
            std::process::exit(1);
        }
    }
}

/// Declares the app's IPC surface and generates a function returning its
/// TypeScript declarations, for `ui-client`'s `typed<…>()` and `ui-pack gen-ts`.
///
/// ```ignore
/// ui::ipc_types! {
///     pub fn ipc_typescript;
///     invoke export_report(req: ExportRequest) -> Report;
///     invoke slice(path: String, start: u32, end: u32) -> String;
///     stream term_open(rows: u16, cols: u16) -> i32, data: Bytes;
/// }
/// ```
///
/// The generated file exports each struct's `interface`, `IpcInvoke` (call
/// name → function type) and `IpcStreams` (stream name → `{ args, data, result }`).
#[macro_export]
macro_rules! ipc_types {
    (
        $vis:vis fn $fname:ident;
        $( $kind:ident $method:ident ( $( $arg:ident : $aty:ty ),* $(,)? ) -> $ret:ty $(, data: $dty:ty)? ; )*
    ) => {
        $vis fn $fname() -> ::std::string::String {
            #[allow(unused_imports)]
            use $crate::ts::{__TsFallback as _, __TsViaType as _};
            let mut decls = ::std::collections::BTreeMap::new();
            let mut invokes = ::std::string::String::new();
            let mut streams = ::std::string::String::new();
            $(
                $( (&$crate::ts::__TsProbe::<$aty>::new()).__ts_declarations(&mut decls); )*
                (&$crate::ts::__TsProbe::<$ret>::new()).__ts_declarations(&mut decls);
                $( (&$crate::ts::__TsProbe::<$dty>::new()).__ts_declarations(&mut decls); )?
                let args: ::std::vec::Vec<::std::string::String> = ::std::vec![
                    $( ::std::format!("{}: {}", ::std::stringify!($arg), (&$crate::ts::__TsProbe::<$aty>::new()).__ts_type()) ),*
                ];
                let ret = (&$crate::ts::__TsProbe::<$ret>::new()).__ts_type();
                $crate::__ipc_types_entry!($kind, $method, args, ret, invokes, streams $(, (&$crate::ts::__TsProbe::<$dty>::new()).__ts_type())?);
            )*
            let mut out = ::std::string::String::from("// @generated by ui::ipc_types!. Do not edit.\n\n");
            for decl in decls.values() {
                out.push_str(decl);
                out.push('\n');
            }
            out.push_str("export interface IpcInvoke {\n");
            out.push_str(&invokes);
            out.push_str("}\n\nexport interface IpcStreams {\n");
            out.push_str(&streams);
            out.push_str("}\n");
            out
        }
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __ipc_types_entry {
    (invoke, $method:ident, $args:ident, $ret:ident, $invokes:ident, $streams:ident) => {
        $invokes.push_str(&::std::format!(
            "  {}({}): {};\n",
            ::std::stringify!($method),
            $args.join(", "),
            $ret
        ));
    };
    (stream, $method:ident, $args:ident, $ret:ident, $invokes:ident, $streams:ident, $data:expr) => {
        $streams.push_str(&::std::format!(
            "  {}: {{ args: [{}]; data: {}; result: {} }};\n",
            ::std::stringify!($method),
            $args.join(", "),
            $data,
            $ret
        ));
    };
    (stream, $method:ident, $args:ident, $ret:ident, $invokes:ident, $streams:ident) => {
        $streams.push_str(&::std::format!(
            "  {}: {{ args: [{}]; data: unknown; result: {} }};\n",
            ::std::stringify!($method),
            $args.join(", "),
            $ret
        ));
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    crate::ipc_struct! {
        struct Page { index: u32, title: Option<String> }
    }

    crate::ipc_struct! {
        struct Report { path: String, pages: Vec<Page>, raw: Bytes, extra: NoTs }
    }

    struct NoTs;
    impl crate::ipc::FromIpc for NoTs {
        fn from_ipc(_: &Value) -> Result<Self, crate::ipc::DecodeError> {
            Ok(NoTs)
        }
    }
    impl crate::ipc::IntoIpc for NoTs {
        fn into_ipc(self) -> Value {
            Value::Null
        }
    }

    crate::ipc_types! {
        fn api;
        invoke export_report(path: String, pages: Option<Vec<u32>>) -> Report;
        invoke ping() -> ();
        stream term_open(rows: u16, cols: u16) -> i32, data: Bytes;
    }

    #[test]
    fn generates_interfaces_and_call_maps() {
        let ts = api();
        assert!(ts.starts_with("// @generated"), "{ts}");
        assert!(
            ts.contains("export interface Page {\n  index: number;\n  title?: string | null;\n}"),
            "{ts}"
        );
        assert!(
            ts.contains("  pages: Page[];\n  raw: Uint8Array;\n  extra: unknown;\n"),
            "{ts}"
        );
        assert!(
            ts.contains("  export_report(path: string, pages: number[] | null): Report;\n"),
            "{ts}"
        );
        assert!(ts.contains("  ping(): null;\n"), "{ts}");
        assert!(ts.contains("  term_open: { args: [rows: number, cols: number]; data: Uint8Array; result: number };\n"), "{ts}");
        assert_eq!(<(String, u8)>::ts_type(), "[string, number]");
        assert_eq!(<Vec<Option<u8>>>::ts_type(), "(number | null)[]");
    }
}
