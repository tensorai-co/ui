//! Zero-crate binary wire codec, RFC 8259 JSON codec, and typed IPC traits/macros.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

/// Dynamic value exchanged across the `ui` host <-> webview binary IPC bridge.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Bytes(Vec<u8>),
    Array(Vec<Value>),
    Object(BTreeMap<String, Value>),
}

impl Value {
    pub fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Self::Int(i) => Some(*i),
            Self::Float(f)
                if f.fract() == 0.0 && *f >= i64::MIN as f64 && *f <= i64::MAX as f64 =>
            {
                Some(*f as i64)
            }
            _ => None,
        }
    }

    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Self::Int(i) if *i >= 0 => Some(*i as u64),
            Self::Float(f) if f.fract() == 0.0 && *f >= 0.0 && *f <= u64::MAX as f64 => {
                Some(*f as u64)
            }
            _ => None,
        }
    }

    pub fn as_usize(&self) -> Option<usize> {
        self.as_u64().and_then(|u| usize::try_from(u).ok())
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Int(i) => Some(*i as f64),
            Self::Float(f) => Some(*f),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(s) => Some(s.as_str()),
            _ => None,
        }
    }

    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Bytes(b) => Some(b.as_slice()),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Self::Array(a) => Some(a.as_slice()),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<&BTreeMap<String, Value>> {
        match self {
            Self::Object(o) => Some(o),
            _ => None,
        }
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.as_object()?.get(key)
    }

    pub fn index(&self, idx: usize) -> Option<&Value> {
        self.as_array()?.get(idx)
    }

    /// Serializes this value into RFC 8259 JSON text.
    /// JSON text for this value. `Bytes` become a JSON number array (`[1,2,3]`,
    /// 3–4× the size); IPC sends them as raw frames instead. Use
    /// [`try_to_json`](Self::try_to_json) where binary must not be widened.
    pub fn to_json(&self) -> String {
        stringify(self)
    }

    /// JSON text for this value, or an error if it contains `Bytes` (which
    /// only travel as binary wire frames).
    pub fn try_to_json(&self) -> Result<String, DecodeError> {
        if self.contains_bytes() {
            return Err(DecodeError::new(
                "value contains bytes; send it over IPC as a binary frame instead of JSON",
            ));
        }
        Ok(stringify(self))
    }

    fn contains_bytes(&self) -> bool {
        match self {
            Self::Bytes(_) => true,
            Self::Array(items) => items.iter().any(Self::contains_bytes),
            Self::Object(map) => map.values().any(Self::contains_bytes),
            _ => false,
        }
    }

    /// Parses RFC 8259 JSON text into a `Value`.
    pub fn from_json(input: &str) -> Result<Self, DecodeError> {
        parse(input)
    }
}

/// Error returned while parsing JSON or converting a `Value` into a strongly typed Rust value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodeError {
    pub message: String,
}

impl DecodeError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for DecodeError {}

/// Decodes a value received from JS (`ipc.invoke` / `ipc.send`).
pub trait FromIpc: Sized {
    fn from_ipc(value: &Value) -> Result<Self, DecodeError>;
}

/// Encodes a value sent back to JS (`ctx.emit`, `webview.emit`, RPC return value).
pub trait IntoIpc {
    fn into_ipc(self) -> Value;
}

/// Return type accepted by `.rpc(...)` closures (`T: IntoIpc` or `ui::Result<T>`).
pub trait RpcResult {
    fn into_rpc_result(self) -> Result<Value, String>;
}

impl<T: IntoIpc> RpcResult for T {
    fn into_rpc_result(self) -> Result<Value, String> {
        Ok(self.into_ipc())
    }
}

impl<T: IntoIpc> RpcResult for crate::Result<T> {
    fn into_rpc_result(self) -> Result<Value, String> {
        self.map(IntoIpc::into_ipc).map_err(|e| e.to_string())
    }
}

impl FromIpc for Value {
    fn from_ipc(value: &Value) -> Result<Self, DecodeError> {
        Ok(value.clone())
    }
}

impl IntoIpc for Value {
    fn into_ipc(self) -> Value {
        self
    }
}

impl FromIpc for () {
    fn from_ipc(value: &Value) -> Result<Self, DecodeError> {
        match value {
            Value::Null => Ok(()),
            Value::Array(items) if items.is_empty() => Ok(()),
            _ => Err(DecodeError::new("expected null or empty array for ()")),
        }
    }
}

impl IntoIpc for () {
    fn into_ipc(self) -> Value {
        Value::Null
    }
}

impl FromIpc for bool {
    fn from_ipc(value: &Value) -> Result<Self, DecodeError> {
        value
            .as_bool()
            .ok_or_else(|| DecodeError::new("expected boolean"))
    }
}

impl IntoIpc for bool {
    fn into_ipc(self) -> Value {
        Value::Bool(self)
    }
}

macro_rules! impl_signed_ipc {
    ($($t:ty),* $(,)?) => {
        $(
            impl FromIpc for $t {
                fn from_ipc(value: &Value) -> Result<Self, DecodeError> {
                    let v = value.as_i64().ok_or_else(|| DecodeError::new(concat!("expected integer for ", stringify!($t))))?;
                    <$t>::try_from(v).map_err(|_| DecodeError::new(concat!("integer out of range for ", stringify!($t))))
                }
            }

            impl IntoIpc for $t {
                fn into_ipc(self) -> Value {
                    Value::Int(i64::from(self))
                }
            }
        )*
    };
}

impl_signed_ipc!(i8, i16, i32, i64);

impl FromIpc for isize {
    fn from_ipc(value: &Value) -> Result<Self, DecodeError> {
        let v = value
            .as_i64()
            .ok_or_else(|| DecodeError::new("expected integer for isize"))?;
        isize::try_from(v).map_err(|_| DecodeError::new("integer out of range for isize"))
    }
}

impl IntoIpc for isize {
    fn into_ipc(self) -> Value {
        Value::Int(self as i64)
    }
}

macro_rules! impl_unsigned_small_ipc {
    ($($t:ty),* $(,)?) => {
        $(
            impl FromIpc for $t {
                fn from_ipc(value: &Value) -> Result<Self, DecodeError> {
                    let v = value.as_u64().ok_or_else(|| DecodeError::new(concat!("expected non-negative integer for ", stringify!($t))))?;
                    <$t>::try_from(v).map_err(|_| DecodeError::new(concat!("integer out of range for ", stringify!($t))))
                }
            }

            impl IntoIpc for $t {
                fn into_ipc(self) -> Value {
                    Value::Int(i64::from(self))
                }
            }
        )*
    };
}

impl_unsigned_small_ipc!(u8, u16, u32);

impl FromIpc for u64 {
    fn from_ipc(value: &Value) -> Result<Self, DecodeError> {
        value
            .as_u64()
            .ok_or_else(|| DecodeError::new("expected non-negative integer for u64"))
    }
}

impl IntoIpc for u64 {
    fn into_ipc(self) -> Value {
        match i64::try_from(self) {
            Ok(i) => Value::Int(i),
            Err(_) => Value::Float(self as f64),
        }
    }
}

impl FromIpc for usize {
    fn from_ipc(value: &Value) -> Result<Self, DecodeError> {
        value
            .as_usize()
            .ok_or_else(|| DecodeError::new("expected non-negative integer for usize"))
    }
}

impl IntoIpc for usize {
    fn into_ipc(self) -> Value {
        (self as u64).into_ipc()
    }
}

impl FromIpc for f32 {
    fn from_ipc(value: &Value) -> Result<Self, DecodeError> {
        value
            .as_f64()
            .map(|v| v as f32)
            .ok_or_else(|| DecodeError::new("expected number for f32"))
    }
}

impl IntoIpc for f32 {
    fn into_ipc(self) -> Value {
        Value::Float(f64::from(self))
    }
}

impl FromIpc for f64 {
    fn from_ipc(value: &Value) -> Result<Self, DecodeError> {
        value
            .as_f64()
            .ok_or_else(|| DecodeError::new("expected number for f64"))
    }
}

impl IntoIpc for f64 {
    fn into_ipc(self) -> Value {
        Value::Float(self)
    }
}

impl FromIpc for String {
    fn from_ipc(value: &Value) -> Result<Self, DecodeError> {
        value
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| DecodeError::new("expected string"))
    }
}

impl IntoIpc for String {
    fn into_ipc(self) -> Value {
        Value::String(self)
    }
}

impl IntoIpc for &str {
    fn into_ipc(self) -> Value {
        Value::String(self.to_owned())
    }
}

impl FromIpc for PathBuf {
    fn from_ipc(value: &Value) -> Result<Self, DecodeError> {
        value
            .as_str()
            .map(PathBuf::from)
            .ok_or_else(|| DecodeError::new("expected path string"))
    }
}

impl IntoIpc for PathBuf {
    fn into_ipc(self) -> Value {
        Value::String(self.to_string_lossy().into_owned())
    }
}

impl IntoIpc for &Path {
    fn into_ipc(self) -> Value {
        Value::String(self.to_string_lossy().into_owned())
    }
}

/// Raw binary payload transmitted across IPC without JSON or base64 encoding.
///
/// Maps directly to `ArrayBuffer` / `Uint8Array` in JS.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Bytes(pub Vec<u8>);

impl From<Vec<u8>> for Bytes {
    fn from(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }
}

impl From<&[u8]> for Bytes {
    fn from(bytes: &[u8]) -> Self {
        Self(bytes.to_vec())
    }
}

impl From<Bytes> for Vec<u8> {
    fn from(bytes: Bytes) -> Self {
        bytes.0
    }
}

impl AsRef<[u8]> for Bytes {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl std::ops::Deref for Bytes {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl FromIpc for Bytes {
    fn from_ipc(value: &Value) -> Result<Self, DecodeError> {
        match value {
            Value::Bytes(bytes) => Ok(Self(bytes.clone())),
            Value::Array(items) => {
                let mut out = Vec::with_capacity(items.len());
                for item in items {
                    out.push(u8::from_ipc(item)?);
                }
                Ok(Self(out))
            }
            _ => Err(DecodeError::new("expected binary bytes")),
        }
    }
}

impl IntoIpc for Bytes {
    fn into_ipc(self) -> Value {
        Value::Bytes(self.0)
    }
}

impl IntoIpc for &[u8] {
    fn into_ipc(self) -> Value {
        Value::Bytes(self.to_vec())
    }
}

impl<const N: usize> IntoIpc for &[u8; N] {
    fn into_ipc(self) -> Value {
        Value::Bytes(self.to_vec())
    }
}

impl<T: FromIpc> FromIpc for Option<T> {
    fn from_ipc(value: &Value) -> Result<Self, DecodeError> {
        if value.is_null() {
            Ok(None)
        } else {
            T::from_ipc(value).map(Some)
        }
    }
}

impl<T: IntoIpc> IntoIpc for Option<T> {
    fn into_ipc(self) -> Value {
        match self {
            Some(v) => v.into_ipc(),
            None => Value::Null,
        }
    }
}

impl<T: FromIpc> FromIpc for Vec<T> {
    fn from_ipc(value: &Value) -> Result<Self, DecodeError> {
        let arr = value
            .as_array()
            .ok_or_else(|| DecodeError::new("expected array"))?;
        arr.iter().map(T::from_ipc).collect()
    }
}

impl<T: IntoIpc> IntoIpc for Vec<T> {
    fn into_ipc(self) -> Value {
        Value::Array(self.into_iter().map(IntoIpc::into_ipc).collect())
    }
}

impl<A: FromIpc> FromIpc for (A,) {
    fn from_ipc(value: &Value) -> Result<Self, DecodeError> {
        match value {
            Value::Array(items) => {
                if items.len() != 1 {
                    return Err(DecodeError::new(format!(
                        "expected 1-element array, got {}",
                        items.len()
                    )));
                }
                Ok((A::from_ipc(&items[0])?,))
            }
            other => Ok((A::from_ipc(other)?,)),
        }
    }
}

impl<A: IntoIpc> IntoIpc for (A,) {
    fn into_ipc(self) -> Value {
        Value::Array(vec![self.0.into_ipc()])
    }
}

impl<A: FromIpc, B: FromIpc> FromIpc for (A, B) {
    fn from_ipc(value: &Value) -> Result<Self, DecodeError> {
        let items = value
            .as_array()
            .ok_or_else(|| DecodeError::new("expected 2-element array"))?;
        if items.len() != 2 {
            return Err(DecodeError::new(format!(
                "expected 2-element array, got {}",
                items.len()
            )));
        }
        Ok((A::from_ipc(&items[0])?, B::from_ipc(&items[1])?))
    }
}

impl<A: IntoIpc, B: IntoIpc> IntoIpc for (A, B) {
    fn into_ipc(self) -> Value {
        Value::Array(vec![self.0.into_ipc(), self.1.into_ipc()])
    }
}

impl<A: FromIpc, B: FromIpc, C: FromIpc> FromIpc for (A, B, C) {
    fn from_ipc(value: &Value) -> Result<Self, DecodeError> {
        let items = value
            .as_array()
            .ok_or_else(|| DecodeError::new("expected 3-element array"))?;
        if items.len() != 3 {
            return Err(DecodeError::new(format!(
                "expected 3-element array, got {}",
                items.len()
            )));
        }
        Ok((
            A::from_ipc(&items[0])?,
            B::from_ipc(&items[1])?,
            C::from_ipc(&items[2])?,
        ))
    }
}

impl<A: IntoIpc, B: IntoIpc, C: IntoIpc> IntoIpc for (A, B, C) {
    fn into_ipc(self) -> Value {
        Value::Array(vec![
            self.0.into_ipc(),
            self.1.into_ipc(),
            self.2.into_ipc(),
        ])
    }
}

impl<A: FromIpc, B: FromIpc, C: FromIpc, D: FromIpc> FromIpc for (A, B, C, D) {
    fn from_ipc(value: &Value) -> Result<Self, DecodeError> {
        let items = value
            .as_array()
            .ok_or_else(|| DecodeError::new("expected 4-element array"))?;
        if items.len() != 4 {
            return Err(DecodeError::new(format!(
                "expected 4-element array, got {}",
                items.len()
            )));
        }
        Ok((
            A::from_ipc(&items[0])?,
            B::from_ipc(&items[1])?,
            C::from_ipc(&items[2])?,
            D::from_ipc(&items[3])?,
        ))
    }
}

impl<A: IntoIpc, B: IntoIpc, C: IntoIpc, D: IntoIpc> IntoIpc for (A, B, C, D) {
    fn into_ipc(self) -> Value {
        Value::Array(vec![
            self.0.into_ipc(),
            self.1.into_ipc(),
            self.2.into_ipc(),
            self.3.into_ipc(),
        ])
    }
}

// ---------------------------------------------------------------------------
// RFC 8259 JSON parser & serializer
// ---------------------------------------------------------------------------

/// Parses an RFC 8259 JSON document into a `Value`.
pub fn parse(input: &str) -> Result<Value, DecodeError> {
    let mut p = Parser {
        bytes: input.as_bytes(),
        pos: 0,
    };
    p.skip_ws();
    let v = p.parse_value()?;
    p.skip_ws();
    if p.pos != p.bytes.len() {
        return Err(p.err("unexpected trailing bytes after JSON value"));
    }
    Ok(v)
}

/// Serializes a `Value` into compact RFC 8259 JSON.
pub fn stringify(value: &Value) -> String {
    let mut out = String::new();
    write_json(&mut out, value);
    out
}

fn write_json(out: &mut String, value: &Value) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Int(i) => {
            use std::fmt::Write as _;
            let _ = write!(out, "{i}");
        }
        Value::Float(f) => {
            if f.is_finite() {
                let s = f.to_string();
                out.push_str(&s);
                if !s.contains(['.', 'e', 'E']) {
                    out.push_str(".0");
                }
            } else {
                out.push_str("null");
            }
        }
        Value::String(s) => write_json_str(out, s),
        Value::Bytes(bytes) => {
            use std::fmt::Write as _;
            out.push('[');
            for (i, byte) in bytes.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                let _ = write!(out, "{byte}");
            }
            out.push(']');
        }
        Value::Array(arr) => {
            out.push('[');
            for (i, item) in arr.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_json(out, item);
            }
            out.push(']');
        }
        Value::Object(map) => {
            out.push('{');
            for (i, (k, v)) in map.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_json_str(out, k);
                out.push(':');
                write_json(out, v);
            }
            out.push('}');
        }
    }
}

pub(crate) fn write_json_str(out: &mut String, s: &str) {
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0C}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                use std::fmt::Write as _;
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn err(&self, msg: &str) -> DecodeError {
        DecodeError::new(format!("{msg} at byte {}", self.pos))
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while let Some(b) = self.peek() {
            if matches!(b, b' ' | b'\t' | b'\r' | b'\n') {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn parse_value(&mut self) -> Result<Value, DecodeError> {
        self.skip_ws();
        match self.peek() {
            Some(b'n') => self.parse_literal(b"null", Value::Null),
            Some(b't') => self.parse_literal(b"true", Value::Bool(true)),
            Some(b'f') => self.parse_literal(b"false", Value::Bool(false)),
            Some(b'"') => self.parse_string().map(Value::String),
            Some(b'[') => self.parse_array(),
            Some(b'{') => self.parse_object(),
            Some(b'-' | b'0'..=b'9') => self.parse_number(),
            _ => Err(self.err("unexpected token while reading JSON value")),
        }
    }

    fn parse_literal(&mut self, lit: &[u8], val: Value) -> Result<Value, DecodeError> {
        if self.bytes[self.pos..].starts_with(lit) {
            self.pos += lit.len();
            Ok(val)
        } else {
            Err(self.err("invalid literal"))
        }
    }

    fn parse_string(&mut self) -> Result<String, DecodeError> {
        if self.peek() != Some(b'"') {
            return Err(self.err("expected opening quote"));
        }
        self.pos += 1;
        let mut out = String::new();
        while self.pos < self.bytes.len() {
            let b = self.bytes[self.pos];
            match b {
                b'"' => {
                    self.pos += 1;
                    return Ok(out);
                }
                b'\\' => {
                    self.pos += 1;
                    let esc = self
                        .peek()
                        .ok_or_else(|| self.err("unterminated escape sequence"))?;
                    self.pos += 1;
                    match esc {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{08}'),
                        b'f' => out.push('\u{0C}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let code = self.parse_hex4()?;
                            if (0xD800..=0xDBFF).contains(&code) {
                                if !self.bytes[self.pos..].starts_with(b"\\u") {
                                    return Err(self.err("unpaired high UTF-16 surrogate"));
                                }
                                self.pos += 2;
                                let low = self.parse_hex4()?;
                                if !(0xDC00..=0xDFFF).contains(&low) {
                                    return Err(self.err("invalid low UTF-16 surrogate"));
                                }
                                let cp = 0x10000
                                    + (((u32::from(code) - 0xD800) << 10)
                                        | (u32::from(low) - 0xDC00));
                                let ch = char::from_u32(cp)
                                    .ok_or_else(|| self.err("invalid Unicode scalar"))?;
                                out.push(ch);
                            } else {
                                let ch = char::from_u32(u32::from(code))
                                    .ok_or_else(|| self.err("invalid UTF-16 code unit"))?;
                                out.push(ch);
                            }
                        }
                        _ => return Err(self.err("invalid JSON string escape")),
                    }
                }
                c if c < 0x20 => return Err(self.err("unescaped control character in JSON string")),
                c if c < 0x80 => {
                    out.push(c as char);
                    self.pos += 1;
                }
                _ => {
                    let rest = std::str::from_utf8(&self.bytes[self.pos..])
                        .map_err(|_| self.err("invalid UTF-8 in JSON string"))?;
                    let ch = rest
                        .chars()
                        .next()
                        .ok_or_else(|| self.err("unexpected EOF"))?;
                    out.push(ch);
                    self.pos += ch.len_utf8();
                }
            }
        }
        Err(self.err("unterminated JSON string"))
    }

    fn parse_hex4(&mut self) -> Result<u16, DecodeError> {
        if self.pos + 4 > self.bytes.len() {
            return Err(self.err("truncated \\uXXXX escape"));
        }
        let hex = std::str::from_utf8(&self.bytes[self.pos..self.pos + 4])
            .map_err(|_| self.err("invalid UTF-8 in \\uXXXX escape"))?;
        self.pos += 4;
        u16::from_str_radix(hex, 16).map_err(|_| self.err("invalid hex in \\uXXXX escape"))
    }

    fn parse_number(&mut self) -> Result<Value, DecodeError> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        match self.peek() {
            Some(b'0') => self.pos += 1,
            Some(b'1'..=b'9') => {
                while matches!(self.peek(), Some(b'0'..=b'9')) {
                    self.pos += 1;
                }
            }
            _ => return Err(self.err("invalid JSON number")),
        }
        let mut is_float = false;
        if self.peek() == Some(b'.') {
            is_float = true;
            self.pos += 1;
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(self.err("expected digit after decimal point"));
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            is_float = true;
            self.pos += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(self.err("expected digit in exponent"));
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }
        let slice = std::str::from_utf8(&self.bytes[start..self.pos])
            .map_err(|_| self.err("invalid UTF-8 in number"))?;
        if !is_float && let Ok(i) = slice.parse::<i64>() {
            return Ok(Value::Int(i));
        }
        let f = slice
            .parse::<f64>()
            .map_err(|_| self.err("invalid float literal"))?;
        if !f.is_finite() {
            return Err(self.err("JSON number overflow"));
        }
        Ok(Value::Float(f))
    }

    fn parse_array(&mut self) -> Result<Value, DecodeError> {
        self.pos += 1; // '['
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(Value::Array(items));
        }
        loop {
            items.push(self.parse_value()?);
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b']') => {
                    self.pos += 1;
                    return Ok(Value::Array(items));
                }
                _ => return Err(self.err("expected ',' or ']' in array")),
            }
        }
    }

    fn parse_object(&mut self) -> Result<Value, DecodeError> {
        self.pos += 1; // '{'
        let mut map = BTreeMap::new();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(Value::Object(map));
        }
        loop {
            self.skip_ws();
            let key = self.parse_string()?;
            self.skip_ws();
            if self.peek() != Some(b':') {
                return Err(self.err("expected ':' after object key"));
            }
            self.pos += 1;
            let val = self.parse_value()?;
            map.insert(key, val);
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(Value::Object(map));
                }
                _ => return Err(self.err("expected ',' or '}' in object")),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Declarative `json!` and `ipc_enum!` macros
// ---------------------------------------------------------------------------

#[doc(hidden)]
#[macro_export]
macro_rules! __json_array {
    ($arr:ident) => {};
    ($arr:ident null) => {
        $arr.push($crate::ipc::Value::Null);
    };
    ($arr:ident null, $($rest:tt)*) => {
        $arr.push($crate::ipc::Value::Null);
        $crate::__json_array!($arr $($rest)*);
    };
    ($arr:ident [$($inner:tt)*]) => {
        $arr.push($crate::json!([$($inner)*]));
    };
    ($arr:ident [$($inner:tt)*], $($rest:tt)*) => {
        $arr.push($crate::json!([$($inner)*]));
        $crate::__json_array!($arr $($rest)*);
    };
    ($arr:ident {$($inner:tt)*}) => {
        $arr.push($crate::json!({$($inner)*}));
    };
    ($arr:ident {$($inner:tt)*}, $($rest:tt)*) => {
        $arr.push($crate::json!({$($inner)*}));
        $crate::__json_array!($arr $($rest)*);
    };
    ($arr:ident $e:expr) => {
        $arr.push($crate::ipc::IntoIpc::into_ipc($e));
    };
    ($arr:ident $e:expr, $($rest:tt)*) => {
        $arr.push($crate::ipc::IntoIpc::into_ipc($e));
        $crate::__json_array!($arr $($rest)*);
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __json_object {
    ($map:ident) => {};
    ($map:ident $k:literal : null) => {
        $map.insert(::std::string::String::from($k), $crate::ipc::Value::Null);
    };
    ($map:ident $k:literal : null, $($rest:tt)*) => {
        $map.insert(::std::string::String::from($k), $crate::ipc::Value::Null);
        $crate::__json_object!($map $($rest)*);
    };
    ($map:ident $k:literal : [$($inner:tt)*]) => {
        $map.insert(::std::string::String::from($k), $crate::json!([$($inner)*]));
    };
    ($map:ident $k:literal : [$($inner:tt)*], $($rest:tt)*) => {
        $map.insert(::std::string::String::from($k), $crate::json!([$($inner)*]));
        $crate::__json_object!($map $($rest)*);
    };
    ($map:ident $k:literal : {$($inner:tt)*}) => {
        $map.insert(::std::string::String::from($k), $crate::json!({$($inner)*}));
    };
    ($map:ident $k:literal : {$($inner:tt)*}, $($rest:tt)*) => {
        $map.insert(::std::string::String::from($k), $crate::json!({$($inner)*}));
        $crate::__json_object!($map $($rest)*);
    };
    ($map:ident $k:literal : $v:expr) => {
        $map.insert(::std::string::String::from($k), $crate::ipc::IntoIpc::into_ipc($v));
    };
    ($map:ident $k:literal : $v:expr, $($rest:tt)*) => {
        $map.insert(::std::string::String::from($k), $crate::ipc::IntoIpc::into_ipc($v));
        $crate::__json_object!($map $($rest)*);
    };
    ($map:ident $k:ident : null) => {
        $map.insert(::std::string::String::from(stringify!($k)), $crate::ipc::Value::Null);
    };
    ($map:ident $k:ident : null, $($rest:tt)*) => {
        $map.insert(::std::string::String::from(stringify!($k)), $crate::ipc::Value::Null);
        $crate::__json_object!($map $($rest)*);
    };
    ($map:ident $k:ident : [$($inner:tt)*]) => {
        $map.insert(::std::string::String::from(stringify!($k)), $crate::json!([$($inner)*]));
    };
    ($map:ident $k:ident : [$($inner:tt)*], $($rest:tt)*) => {
        $map.insert(::std::string::String::from(stringify!($k)), $crate::json!([$($inner)*]));
        $crate::__json_object!($map $($rest)*);
    };
    ($map:ident $k:ident : {$($inner:tt)*}) => {
        $map.insert(::std::string::String::from(stringify!($k)), $crate::json!({$($inner)*}));
    };
    ($map:ident $k:ident : {$($inner:tt)*}, $($rest:tt)*) => {
        $map.insert(::std::string::String::from(stringify!($k)), $crate::json!({$($inner)*}));
        $crate::__json_object!($map $($rest)*);
    };
    ($map:ident $k:ident : $v:expr) => {
        $map.insert(::std::string::String::from(stringify!($k)), $crate::ipc::IntoIpc::into_ipc($v));
    };
    ($map:ident $k:ident : $v:expr, $($rest:tt)*) => {
        $map.insert(::std::string::String::from(stringify!($k)), $crate::ipc::IntoIpc::into_ipc($v));
        $crate::__json_object!($map $($rest)*);
    };
}

/// Constructs an `ipc::Value` inline without third-party dependencies.
#[macro_export]
macro_rules! json {
    (null) => {
        $crate::ipc::Value::Null
    };
    ([]) => {
        $crate::ipc::Value::Array(::std::vec::Vec::new())
    };
    ([$($tt:tt)+]) => {{
        // `identity` keeps clippy's vec-init-then-push lint out of callers' code.
        let mut __arr = ::std::convert::identity(::std::vec::Vec::new());
        $crate::__json_array!(__arr $($tt)+);
        $crate::ipc::Value::Array(__arr)
    }};
    ({}) => {
        $crate::ipc::Value::Object(::std::collections::BTreeMap::new())
    };
    ({$($tt:tt)+}) => {{
        let mut __map = ::std::collections::BTreeMap::new();
        $crate::__json_object!(__map $($tt)+);
        $crate::ipc::Value::Object(__map)
    }};
    ($e:expr) => {
        $crate::ipc::IntoIpc::into_ipc($e)
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __ipc_enum_parse {
    (
        [$($meta:meta)*] [$vis:vis] [$name:ident] [$($decls:tt)*] [$($enc:tt)*] [$($parsed:tt)*]
        #[tag = $tag:literal] $v:ident { $($f:ident : $t:ty),* $(,)? }, $($rest:tt)*
    ) => {
        $crate::__ipc_enum_parse!(
            [$($meta)*] [$vis] [$name]
            [$($decls)* $v { $($f : $t),* },]
            [$($enc)*
                $name::$v { $($f),* } => {
                    let mut m = ::std::collections::BTreeMap::new();
                    m.insert(::std::string::String::from("type"), $crate::ipc::Value::String(::std::string::String::from($tag)));
                    $(
                        m.insert(::std::string::String::from(stringify!($f)), $crate::ipc::IntoIpc::into_ipc($f));
                    )*
                    m
                }
            ]
            [$($parsed)* { kind = struct, name = $v, tag = $tag, fields = [ $($f : $t),* ] }]
            $($rest)*
        );
    };
    (
        [$($meta:meta)*] [$vis:vis] [$name:ident] [$($decls:tt)*] [$($enc:tt)*] [$($parsed:tt)*]
        #[tag = $tag:literal] $v:ident { $($f:ident : $t:ty),* $(,)? }
    ) => {
        $crate::__ipc_enum_parse!(
            [$($meta)*] [$vis] [$name]
            [$($decls)* $v { $($f : $t),* },]
            [$($enc)*
                $name::$v { $($f),* } => {
                    let mut m = ::std::collections::BTreeMap::new();
                    m.insert(::std::string::String::from("type"), $crate::ipc::Value::String(::std::string::String::from($tag)));
                    $(
                        m.insert(::std::string::String::from(stringify!($f)), $crate::ipc::IntoIpc::into_ipc($f));
                    )*
                    m
                }
            ]
            [$($parsed)* { kind = struct, name = $v, tag = $tag, fields = [ $($f : $t),* ] }]
        );
    };
    (
        [$($meta:meta)*] [$vis:vis] [$name:ident] [$($decls:tt)*] [$($enc:tt)*] [$($parsed:tt)*]
        #[tag = $tag:literal] $v:ident ($t:ty), $($rest:tt)*
    ) => {
        $crate::__ipc_enum_parse!(
            [$($meta)*] [$vis] [$name]
            [$($decls)* $v($t),]
            [$($enc)*
                $name::$v(inner) => {
                    let mut m = ::std::collections::BTreeMap::new();
                    m.insert(::std::string::String::from("type"), $crate::ipc::Value::String(::std::string::String::from($tag)));
                    m.insert(::std::string::String::from("value"), $crate::ipc::IntoIpc::into_ipc(inner));
                    m
                }
            ]
            [$($parsed)* { kind = tuple, name = $v, tag = $tag, ty = $t }]
            $($rest)*
        );
    };
    (
        [$($meta:meta)*] [$vis:vis] [$name:ident] [$($decls:tt)*] [$($enc:tt)*] [$($parsed:tt)*]
        #[tag = $tag:literal] $v:ident ($t:ty)
    ) => {
        $crate::__ipc_enum_parse!(
            [$($meta)*] [$vis] [$name]
            [$($decls)* $v($t),]
            [$($enc)*
                $name::$v(inner) => {
                    let mut m = ::std::collections::BTreeMap::new();
                    m.insert(::std::string::String::from("type"), $crate::ipc::Value::String(::std::string::String::from($tag)));
                    m.insert(::std::string::String::from("value"), $crate::ipc::IntoIpc::into_ipc(inner));
                    m
                }
            ]
            [$($parsed)* { kind = tuple, name = $v, tag = $tag, ty = $t }]
        );
    };
    (
        [$($meta:meta)*] [$vis:vis] [$name:ident] [$($decls:tt)*] [$($enc:tt)*] [$($parsed:tt)*]
        #[tag = $tag:literal] $v:ident, $($rest:tt)*
    ) => {
        $crate::__ipc_enum_parse!(
            [$($meta)*] [$vis] [$name]
            [$($decls)* $v,]
            [$($enc)*
                $name::$v => {
                    let mut m = ::std::collections::BTreeMap::new();
                    m.insert(::std::string::String::from("type"), $crate::ipc::Value::String(::std::string::String::from($tag)));
                    m
                }
            ]
            [$($parsed)* { kind = unit, name = $v, tag = $tag }]
            $($rest)*
        );
    };
    (
        [$($meta:meta)*] [$vis:vis] [$name:ident] [$($decls:tt)*] [$($enc:tt)*] [$($parsed:tt)*]
        #[tag = $tag:literal] $v:ident
    ) => {
        $crate::__ipc_enum_parse!(
            [$($meta)*] [$vis] [$name]
            [$($decls)* $v,]
            [$($enc)*
                $name::$v => {
                    let mut m = ::std::collections::BTreeMap::new();
                    m.insert(::std::string::String::from("type"), $crate::ipc::Value::String(::std::string::String::from($tag)));
                    m
                }
            ]
            [$($parsed)* { kind = unit, name = $v, tag = $tag }]
        );
    };
    (
        [$($meta:meta)*] [$vis:vis] [$name:ident] [$($decls:tt)*] [$($enc:tt)*]
        [ $({ kind = $kind:ident, name = $v:ident, tag = $tag:literal $(, fields = [ $($f:ident : $t:ty),* ])? $(, ty = $tt:ty)? })* ]
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq)]
        $vis enum $name {
            $($decls)*
        }

        impl $crate::ipc::FromIpc for $name {
            fn from_ipc(value: &$crate::ipc::Value) -> ::std::result::Result<Self, $crate::ipc::DecodeError> {
                let tag = value
                    .get("type")
                    .or_else(|| value.get("tag"))
                    .and_then($crate::ipc::Value::as_str)
                    .ok_or_else(|| $crate::ipc::DecodeError::new("expected tagged IPC object with string \"type\""))?;
                match tag {
                    $(
                        $tag => $crate::__ipc_enum_decode!(value, $name, $v, kind = $kind $(, fields = [ $($f : $t),* ])? $(, ty = $tt)?),
                    )*
                    other => ::std::result::Result::Err($crate::ipc::DecodeError::new(::std::format!("unknown IPC enum tag: {other}"))),
                }
            }
        }

        impl $crate::ipc::IntoIpc for $name {
            fn into_ipc(self) -> $crate::ipc::Value {
                let map = match self {
                    $($enc)*
                };
                $crate::ipc::Value::Object(map)
            }
        }
    };
}

/// Defines a tagged IPC enum implementing `FromIpc` and `IntoIpc` without proc-macros.
#[macro_export]
macro_rules! ipc_enum {
    ($(#[$meta:meta])* $vis:vis enum $name:ident { $($body:tt)* }) => {
        $crate::__ipc_enum_parse!([$($meta)*] [$vis] [$name] [] [] [] $($body)*);
    };
}

/// Declares a struct that crosses IPC as a JS object, with `FromIpc` and `IntoIpc`
/// generated field by field.
///
/// ```ignore
/// ui::ipc_struct! {
///     #[derive(Debug, Clone)]
///     pub struct ExportRequest { pub path: String, pub pages: Option<u32> }
/// }
/// ```
///
/// A missing field decodes as `null`, so `Option<T>` fields are optional and every
/// other field reports `missing IPC field "name"`. Decode errors name the field.
#[macro_export]
macro_rules! ipc_struct {
    (
        $(#[$meta:meta])*
        $vis:vis struct $name:ident {
            $( $(#[$fmeta:meta])* $fvis:vis $field:ident : $ty:ty ),* $(,)?
        }
    ) => {
        $(#[$meta])*
        $vis struct $name {
            $( $(#[$fmeta])* $fvis $field : $ty ),*
        }

        impl $crate::ipc::FromIpc for $name {
            fn from_ipc(value: &$crate::ipc::Value) -> ::std::result::Result<Self, $crate::ipc::DecodeError> {
                if !::std::matches!(value, $crate::ipc::Value::Object(_)) {
                    return ::std::result::Result::Err($crate::ipc::DecodeError::new(
                        ::std::concat!("expected an object for ", ::std::stringify!($name)),
                    ));
                }
                ::std::result::Result::Ok(Self {
                    $(
                        $field: match value.get(::std::stringify!($field)) {
                            ::std::option::Option::Some(v) => <$ty as $crate::ipc::FromIpc>::from_ipc(v).map_err(|e| {
                                $crate::ipc::DecodeError::new(::std::format!(
                                    ::std::concat!("IPC field \"", ::std::stringify!($field), "\": {}"),
                                    e
                                ))
                            })?,
                            ::std::option::Option::None => <$ty as $crate::ipc::FromIpc>::from_ipc(&$crate::ipc::Value::Null)
                                .map_err(|_| $crate::ipc::DecodeError::new(
                                    ::std::concat!("missing IPC field \"", ::std::stringify!($field), "\""),
                                ))?,
                        },
                    )*
                })
            }
        }

        impl $crate::ts::TsType for $name {
            fn ts_type() -> ::std::string::String {
                ::std::string::String::from(::std::stringify!($name))
            }
            fn ts_declarations(out: &mut ::std::collections::BTreeMap<::std::string::String, ::std::string::String>) {
                #[allow(unused_imports)]
                use $crate::ts::{__TsFallback as _, __TsViaType as _};
                let key = ::std::string::String::from(::std::stringify!($name));
                if out.contains_key(&key) {
                    return;
                }
                // Placeholder first, so recursive types terminate.
                out.insert(key.clone(), ::std::string::String::new());
                #[allow(unused_mut)]
                let mut body = ::std::string::String::new();
                $(
                    (&$crate::ts::__TsProbe::<$ty>::new()).__ts_declarations(out);
                    let ty = (&$crate::ts::__TsProbe::<$ty>::new()).__ts_type();
                    let opt = if $crate::ts::__is_optional(&ty) { "?" } else { "" };
                    body.push_str(&::std::format!("  {}{}: {};\n", ::std::stringify!($field), opt, ty));
                )*
                out.insert(key, ::std::format!("export interface {} {{\n{}}}\n", ::std::stringify!($name), body));
            }
        }

        impl $crate::ipc::IntoIpc for $name {
            fn into_ipc(self) -> $crate::ipc::Value {
                #[allow(unused_mut)]
                let mut map = ::std::collections::BTreeMap::new();
                $(
                    map.insert(
                        ::std::string::String::from(::std::stringify!($field)),
                        $crate::ipc::IntoIpc::into_ipc(self.$field),
                    );
                )*
                $crate::ipc::Value::Object(map)
            }
        }
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __ipc_enum_decode {
    ($val:ident, $name:ident, $v:ident, kind = struct, fields = [ $($f:ident : $t:ty),* ]) => {{
        ::std::result::Result::Ok($name::$v {
            $(
                $f: <$t as $crate::ipc::FromIpc>::from_ipc(
                    $val.get(stringify!($f))
                        .ok_or_else(|| $crate::ipc::DecodeError::new(concat!("missing IPC field \"", stringify!($f), "\"")))?
                )?,
            )*
        })
    }};
    ($val:ident, $name:ident, $v:ident, kind = tuple, ty = $t:ty) => {{
        let inner = $val
            .get("value")
            .ok_or_else(|| $crate::ipc::DecodeError::new("missing IPC field \"value\""))?;
        ::std::result::Result::Ok($name::$v(<$t as $crate::ipc::FromIpc>::from_ipc(inner)?))
    }};
    ($val:ident, $name:ident, $v:ident, kind = unit) => {
        ::std::result::Result::Ok($name::$v)
    };
}

const WIRE_NULL: u8 = 0x00;
const WIRE_JSON_UTF8: u8 = 0x01;
const WIRE_BYTES: u8 = 0x02;
const WIRE_ARRAY: u8 = 0x03;
const WIRE_OBJECT: u8 = 0x04;
const MAX_WIRE_DEPTH: usize = 64;

const SEND_OP_EVENT: u8 = 0x01;
const SEND_OP_MSG: u8 = 0x02;
const SEND_OP_DRAG: u8 = 0x03;
const SEND_OP_TITLEBAR_DOUBLE_CLICK: u8 = 0x04;
const SEND_OP_CANCEL: u8 = 0x05;
const SEND_OP_STREAM_DATA: u8 = 0x06;
const SEND_OP_FILE_DROP: u8 = 0x07;

/// RPC flag: a little-endian `u32` call id follows the flags byte.
const RPC_FLAG_CALL_ID: u8 = 0x01;
/// RPC flag: the call is an `ipc.stream(..)`; requires `RPC_FLAG_CALL_ID`.
const RPC_FLAG_STREAM: u8 = 0x02;

/// Per-call metadata carried by an RPC packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct CallSpec {
    /// Page-assigned id, present when the call can be cancelled or is a stream.
    pub id: Option<u32>,
    /// True for `ipc.stream(..)` calls.
    pub stream: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ClientSendPacket {
    Event(String, Value),
    Msg(Value),
    Drag,
    /// A double click on a `data-ui-drag` region: zoom or minimize, per the OS setting.
    TitlebarDoubleClick,
    /// The page aborted, timed out, or closed the call with this id.
    Cancel(u32),
    /// Data the page wrote into an open `ipc.stream(..)` with this id.
    StreamData(u32, Value),
    /// The page saw files dropped at `{ x, y }` (CSS pixels).
    FileDrop(Value),
}

struct WireCursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> WireCursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.pos)
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], DecodeError> {
        let end = self
            .pos
            .checked_add(len)
            .ok_or_else(|| DecodeError::new("binary IPC offset overflow"))?;
        if end > self.bytes.len() {
            return Err(DecodeError::new("unexpected end of binary IPC frame"));
        }
        let slice = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    fn read_u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.take(1)?[0])
    }

    fn read_u16_le(&mut self) -> Result<u16, DecodeError> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    fn read_u32_le(&mut self) -> Result<u32, DecodeError> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn read_utf8(&mut self, len: usize) -> Result<&'a str, DecodeError> {
        let slice = self.take(len)?;
        std::str::from_utf8(slice)
            .map_err(|_| DecodeError::new("invalid UTF-8 in binary IPC frame"))
    }

    fn read_rest(&mut self) -> &'a [u8] {
        let slice = &self.bytes[self.pos..];
        self.pos = self.bytes.len();
        slice
    }
}

fn contains_bytes(value: &Value) -> bool {
    match value {
        Value::Bytes(_) => true,
        Value::Array(arr) => arr.iter().any(contains_bytes),
        Value::Object(map) => map.values().any(contains_bytes),
        _ => false,
    }
}

fn write_u32_len_patched(out: &mut Vec<u8>, f: impl FnOnce(&mut Vec<u8>)) {
    let len_pos = out.len();
    out.extend_from_slice(&[0u8; 4]);
    let start = out.len();
    f(out);
    let item_len = u32::try_from(out.len() - start).expect("IPC payload exceeds 4 GiB");
    out[len_pos..len_pos + 4].copy_from_slice(&item_len.to_le_bytes());
}

fn write_wire_value(out: &mut Vec<u8>, value: &Value) {
    match value {
        Value::Null => out.push(WIRE_NULL),
        Value::Bytes(bytes) => {
            out.push(WIRE_BYTES);
            out.extend_from_slice(bytes);
        }
        Value::Array(arr) if contains_bytes(value) => {
            out.push(WIRE_ARRAY);
            let count = u32::try_from(arr.len()).expect("IPC array exceeds 4 GiB elements");
            out.extend_from_slice(&count.to_le_bytes());
            for item in arr {
                write_u32_len_patched(out, |buf| write_wire_value(buf, item));
            }
        }
        Value::Object(map) if contains_bytes(value) => {
            out.push(WIRE_OBJECT);
            let count = u32::try_from(map.len()).expect("IPC object exceeds 4 GiB keys");
            out.extend_from_slice(&count.to_le_bytes());
            for (k, v) in map {
                let key_bytes = k.as_bytes();
                let key_len = u32::try_from(key_bytes.len()).expect("IPC key exceeds 4 GiB");
                out.extend_from_slice(&key_len.to_le_bytes());
                out.extend_from_slice(key_bytes);
                write_u32_len_patched(out, |buf| write_wire_value(buf, v));
            }
        }
        other => {
            out.push(WIRE_JSON_UTF8);
            let mut json = String::new();
            write_json(&mut json, other);
            out.extend_from_slice(json.as_bytes());
        }
    }
}

#[cfg(test)]
pub(crate) fn encode_wire_value(value: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    write_wire_value(&mut out, value);
    out
}

pub(crate) fn decode_wire_value(bytes: &[u8]) -> Result<Value, DecodeError> {
    decode_wire_value_depth(bytes, 0)
}

fn decode_wire_value_depth(bytes: &[u8], depth: usize) -> Result<Value, DecodeError> {
    if depth > MAX_WIRE_DEPTH {
        return Err(DecodeError::new("binary IPC nesting limit exceeded"));
    }
    if bytes.is_empty() {
        return Err(DecodeError::new("empty binary IPC value"));
    }
    let mut cur = WireCursor::new(bytes);
    match cur.read_u8()? {
        WIRE_NULL => {
            if cur.remaining() != 0 {
                return Err(DecodeError::new(
                    "trailing bytes after null binary IPC value",
                ));
            }
            Ok(Value::Null)
        }
        WIRE_JSON_UTF8 => {
            let text = std::str::from_utf8(cur.read_rest())
                .map_err(|_| DecodeError::new("invalid UTF-8 JSON payload in binary IPC frame"))?;
            parse(text)
        }
        WIRE_BYTES => Ok(Value::Bytes(cur.read_rest().to_vec())),
        WIRE_ARRAY => {
            let count = usize::try_from(cur.read_u32_le()?)
                .map_err(|_| DecodeError::new("wire array length overflow"))?;
            if count.saturating_mul(5) > cur.remaining() {
                return Err(DecodeError::new("truncated wire array in binary IPC frame"));
            }
            let mut items = Vec::with_capacity(count);
            for _ in 0..count {
                let item_len = usize::try_from(cur.read_u32_le()?)
                    .map_err(|_| DecodeError::new("wire item length overflow"))?;
                let slice = cur.take(item_len)?;
                items.push(decode_wire_value_depth(slice, depth + 1)?);
            }
            if cur.remaining() != 0 {
                return Err(DecodeError::new("trailing bytes after wire array"));
            }
            Ok(Value::Array(items))
        }
        WIRE_OBJECT => {
            let count = usize::try_from(cur.read_u32_le()?)
                .map_err(|_| DecodeError::new("wire object length overflow"))?;
            if count.saturating_mul(9) > cur.remaining() {
                return Err(DecodeError::new(
                    "truncated wire object in binary IPC frame",
                ));
            }
            let mut map = BTreeMap::new();
            for _ in 0..count {
                let key_len = usize::try_from(cur.read_u32_le()?)
                    .map_err(|_| DecodeError::new("wire key length overflow"))?;
                let key = cur.read_utf8(key_len)?.to_owned();
                let val_len = usize::try_from(cur.read_u32_le()?)
                    .map_err(|_| DecodeError::new("wire value length overflow"))?;
                let slice = cur.take(val_len)?;
                map.insert(key, decode_wire_value_depth(slice, depth + 1)?);
            }
            if cur.remaining() != 0 {
                return Err(DecodeError::new("trailing bytes after wire object"));
            }
            Ok(Value::Object(map))
        }
        tag => Err(DecodeError::new(format!(
            "unknown binary IPC tag 0x{tag:02x}"
        ))),
    }
}

pub(crate) fn decode_rpc_request(bytes: &[u8]) -> Result<(String, Value, CallSpec), DecodeError> {
    let mut cur = WireCursor::new(bytes);
    let flags = cur.read_u8()?;
    if flags & !(RPC_FLAG_CALL_ID | RPC_FLAG_STREAM) != 0
        || (flags & RPC_FLAG_STREAM != 0 && flags & RPC_FLAG_CALL_ID == 0)
    {
        return Err(DecodeError::new(format!(
            "unsupported binary RPC flags 0x{flags:02x}"
        )));
    }
    let id = if flags & RPC_FLAG_CALL_ID != 0 {
        Some(cur.read_u32_le()?)
    } else {
        None
    };
    let method_len = usize::from(cur.read_u16_le()?);
    let method = cur.read_utf8(method_len)?.to_owned();
    let arg = decode_wire_value(cur.read_rest())?;
    let call = CallSpec {
        id,
        stream: flags & RPC_FLAG_STREAM != 0,
    };
    Ok((method, arg, call))
}

pub(crate) fn encode_rpc_response(result: Result<Value, String>) -> Vec<u8> {
    let mut out = Vec::new();
    match result {
        Ok(val) => {
            out.push(0x00);
            write_wire_value(&mut out, &val);
        }
        Err(err) => {
            out.push(0x01);
            out.extend_from_slice(err.as_bytes());
        }
    }
    out
}

pub(crate) fn decode_send_packet(bytes: &[u8]) -> Result<ClientSendPacket, DecodeError> {
    let mut cur = WireCursor::new(bytes);
    match cur.read_u8()? {
        SEND_OP_EVENT => {
            let name_len = usize::from(cur.read_u16_le()?);
            let name = cur.read_utf8(name_len)?.to_owned();
            let payload = decode_wire_value(cur.read_rest())?;
            Ok(ClientSendPacket::Event(name, payload))
        }
        SEND_OP_MSG => {
            let payload = decode_wire_value(cur.read_rest())?;
            Ok(ClientSendPacket::Msg(payload))
        }
        SEND_OP_DRAG => {
            if cur.remaining() != 0 {
                return Err(DecodeError::new("trailing bytes in drag packet"));
            }
            Ok(ClientSendPacket::Drag)
        }
        SEND_OP_TITLEBAR_DOUBLE_CLICK => {
            if cur.remaining() != 0 {
                return Err(DecodeError::new("trailing bytes in title-bar packet"));
            }
            Ok(ClientSendPacket::TitlebarDoubleClick)
        }
        SEND_OP_CANCEL => {
            let id = cur.read_u32_le()?;
            if cur.remaining() != 0 {
                return Err(DecodeError::new("trailing bytes in cancel packet"));
            }
            Ok(ClientSendPacket::Cancel(id))
        }
        SEND_OP_STREAM_DATA => {
            let id = cur.read_u32_le()?;
            let payload = decode_wire_value(cur.read_rest())?;
            Ok(ClientSendPacket::StreamData(id, payload))
        }
        SEND_OP_FILE_DROP => Ok(ClientSendPacket::FileDrop(decode_wire_value(
            cur.read_rest(),
        )?)),
        op => Err(DecodeError::new(format!(
            "unknown binary send opcode 0x{op:02x}"
        ))),
    }
}

pub(crate) fn encode_event_frame(channel: &str, payload: &Value) -> Vec<u8> {
    let ch_bytes = channel.as_bytes();
    let ch_len = u16::try_from(ch_bytes.len()).expect("IPC event name exceeds 65535 bytes");
    let mut out = Vec::with_capacity(6 + ch_bytes.len());
    out.extend_from_slice(&ch_len.to_le_bytes());
    out.extend_from_slice(ch_bytes);
    write_u32_len_patched(&mut out, |buf| write_wire_value(buf, payload));
    out
}

pub(crate) fn encode_event_batch(frames: &[Vec<u8>]) -> Vec<u8> {
    if frames.is_empty() {
        return Vec::new();
    }
    let total = 4usize + frames.iter().map(Vec::len).sum::<usize>();
    let mut out = Vec::with_capacity(total);
    let count = u32::try_from(frames.len()).expect("IPC event batch exceeds 4 GiB");
    out.extend_from_slice(&count.to_le_bytes());
    for frame in frames {
        out.extend_from_slice(frame);
    }
    out
}

pub(crate) fn encode_base64(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = u32::from(*chunk.get(1).unwrap_or(&0));
        let b2 = u32::from(*chunk.get(2).unwrap_or(&0));
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(char::from(TABLE[((n >> 18) & 0x3f) as usize]));
        out.push(char::from(TABLE[((n >> 12) & 0x3f) as usize]));
        out.push(if chunk.len() > 1 {
            char::from(TABLE[((n >> 6) & 0x3f) as usize])
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            char::from(TABLE[(n & 0x3f) as usize])
        } else {
            '='
        });
    }
    out
}

pub(crate) fn decode_base64(input: &str) -> Result<Vec<u8>, DecodeError> {
    let bytes: Vec<u8> = input.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    if !bytes.len().is_multiple_of(4) {
        return Err(DecodeError::new("invalid base64 length"));
    }
    let mut out = Vec::with_capacity((bytes.len() / 4) * 3);
    let (quads, _) = bytes.as_chunks::<4>();
    for (idx, chunk) in quads.iter().enumerate() {
        let is_last = (idx + 1) * 4 == bytes.len();
        let mut vals = [0u8; 4];
        let mut pad = 0usize;
        for (i, &b) in chunk.iter().enumerate() {
            if b == b'=' {
                if !is_last || i < 2 {
                    return Err(DecodeError::new("unexpected base64 padding"));
                }
                pad += 1;
                vals[i] = 0;
            } else {
                if pad > 0 {
                    return Err(DecodeError::new("non-padding byte after base64 padding"));
                }
                vals[i] = match b {
                    b'A'..=b'Z' => b - b'A',
                    b'a'..=b'z' => b - b'a' + 26,
                    b'0'..=b'9' => b - b'0' + 52,
                    b'+' => 62,
                    b'/' => 63,
                    _ => return Err(DecodeError::new("invalid base64 byte")),
                };
            }
        }
        let n = (u32::from(vals[0]) << 18)
            | (u32::from(vals[1]) << 12)
            | (u32::from(vals[2]) << 6)
            | u32::from(vals[3]);
        out.push(((n >> 16) & 0xff) as u8);
        if pad < 2 {
            out.push(((n >> 8) & 0xff) as u8);
        }
        if pad == 0 {
            out.push((n & 0xff) as u8);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {

    crate::ipc_struct! {
        #[derive(Debug, Clone, PartialEq)]
        struct ExportRequest {
            path: String,
            pages: Option<u32>,
        }
    }

    #[test]
    fn ipc_struct_round_trip_and_errors() {
        let req = ExportRequest {
            path: "a.pdf".into(),
            pages: Some(3),
        };
        let val = req.clone().into_ipc();
        assert_eq!(ExportRequest::from_ipc(&val).unwrap(), req);

        let mut only_path = std::collections::BTreeMap::new();
        only_path.insert("path".to_owned(), Value::String("b.pdf".into()));
        let decoded = ExportRequest::from_ipc(&Value::Object(only_path)).unwrap();
        assert_eq!(decoded.pages, None);

        let err = ExportRequest::from_ipc(&Value::Object(Default::default())).unwrap_err();
        assert!(
            err.to_string().contains("missing IPC field \"path\""),
            "{err}"
        );
        assert!(ExportRequest::from_ipc(&Value::Null).is_err());
    }

    #[test]
    fn try_to_json_refuses_bytes() {
        let v = crate::json!({ "a": [1, "x"] });
        assert_eq!(v.try_to_json().unwrap(), v.to_json());
        let nested = Value::Object(std::collections::BTreeMap::from([(
            "data".to_owned(),
            Value::Array(vec![Value::Bytes(vec![1])]),
        )]));
        assert!(nested.try_to_json().is_err());
        assert_eq!(nested.to_json(), r#"{"data":[[1]]}"#);
    }

    #[test]
    fn titlebar_double_click_packet() {
        assert_eq!(
            decode_send_packet(&[0x04]).unwrap(),
            ClientSendPacket::TitlebarDoubleClick
        );
        assert!(decode_send_packet(&[0x04, 0x00]).is_err());
    }

    #[test]
    fn call_id_stream_and_cancel_packets() {
        let mut rpc = vec![0x03];
        rpc.extend_from_slice(&7u32.to_le_bytes());
        rpc.extend_from_slice(&2u16.to_le_bytes());
        rpc.extend_from_slice(b"go");
        rpc.push(WIRE_NULL);
        let (method, arg, call) = decode_rpc_request(&rpc).unwrap();
        assert_eq!((method.as_str(), arg), ("go", Value::Null));
        assert_eq!(
            call,
            CallSpec {
                id: Some(7),
                stream: true
            }
        );

        // A stream without a call id, and unknown flags, are rejected.
        assert!(decode_rpc_request(&[0x02, 0, 0, WIRE_NULL]).is_err());
        assert!(decode_rpc_request(&[0x80, 0, 0, WIRE_NULL]).is_err());

        let mut cancel = vec![0x05];
        cancel.extend_from_slice(&9u32.to_le_bytes());
        assert_eq!(
            decode_send_packet(&cancel).unwrap(),
            ClientSendPacket::Cancel(9)
        );
        assert!(decode_send_packet(&[0x05, 1, 0]).is_err());

        let mut data = vec![0x06];
        data.extend_from_slice(&9u32.to_le_bytes());
        data.extend_from_slice(&[WIRE_BYTES, 1, 2]);
        assert_eq!(
            decode_send_packet(&data).unwrap(),
            ClientSendPacket::StreamData(9, Value::Bytes(vec![1, 2]))
        );
    }

    use super::*;

    #[test]
    fn json_roundtrip_rfc8259() {
        let input = r#"{"ok":true,"empty":null,"num":-42,"exp":1.5e2,"arr":[1,"a\nb\u0041","\uD83D\uDE80"]}"#;
        let parsed = parse(input).expect("valid JSON");
        assert_eq!(parsed.get("ok").and_then(Value::as_bool), Some(true));
        assert!(parsed.get("empty").is_some_and(Value::is_null));
        assert_eq!(parsed.get("num").and_then(Value::as_i64), Some(-42));
        assert_eq!(parsed.get("exp").and_then(Value::as_f64), Some(150.0));
        assert_eq!(
            parsed
                .get("arr")
                .and_then(|v| v.index(1))
                .and_then(Value::as_str),
            Some("a\nbA")
        );
        assert_eq!(
            parsed
                .get("arr")
                .and_then(|v| v.index(2))
                .and_then(Value::as_str),
            Some("🚀")
        );

        let roundtrip = parse(&parsed.to_json()).expect("roundtrip JSON");
        assert_eq!(parsed, roundtrip);
    }

    #[test]
    fn json_rejects_invalid_inputs() {
        assert!(parse("").is_err());
        assert!(parse("{a: 1}").is_err());
        assert!(parse("[1, 2").is_err());
        assert!(parse("123 trailing").is_err());
        assert!(parse(r#""\uD800""#).is_err());
    }

    #[test]
    fn tuples_and_primitives() {
        let v = ("path/to/file".to_owned(), 10usize, 512usize).into_ipc();
        let decoded = <(String, usize, usize)>::from_ipc(&v).expect("tuple decode");
        assert_eq!(decoded, ("path/to/file".to_owned(), 10, 512));

        let single = Value::String("abc".into());
        assert_eq!(<(String,)>::from_ipc(&single).unwrap(), ("abc".into(),));
        assert_eq!(<()>::from_ipc(&Value::Null).unwrap(), ());
    }

    #[test]
    fn binary_wire_roundtrip_and_bytes() {
        let raw = vec![0u8, 1, 2, 127, 128, 255];
        let val = ("frame.rgba".to_owned(), Bytes(raw.clone()), 4u32).into_ipc();
        let encoded = encode_wire_value(&val);
        assert_eq!(encoded[0], WIRE_ARRAY);
        let decoded = decode_wire_value(&encoded).expect("wire array roundtrip");
        let (name, bytes, w) =
            <(String, Bytes, u32)>::from_ipc(&decoded).expect("tuple with Bytes");
        assert_eq!(name, "frame.rgba");
        assert_eq!(bytes.0, raw);
        assert_eq!(w, 4);

        let direct = encode_wire_value(&Value::Bytes(vec![9, 8, 7]));
        assert_eq!(direct, vec![WIRE_BYTES, 9, 8, 7]);
        assert_eq!(
            decode_wire_value(&direct).unwrap(),
            Value::Bytes(vec![9, 8, 7])
        );

        let mut map = BTreeMap::new();
        map.insert("id".to_owned(), Value::Int(7));
        map.insert("buf".to_owned(), Value::Bytes(vec![0xde, 0xad, 0xbe, 0xef]));
        let obj = Value::Object(map);
        let obj_enc = encode_wire_value(&obj);
        assert_eq!(obj_enc[0], WIRE_OBJECT);
        assert_eq!(decode_wire_value(&obj_enc).unwrap(), obj);
    }

    #[test]
    fn binary_packets_and_base64() {
        let mut rpc = Vec::new();
        rpc.push(0x00);
        rpc.extend_from_slice(&4u16.to_le_bytes());
        rpc.extend_from_slice(b"echo");
        rpc.extend_from_slice(&encode_wire_value(&Value::Bytes(vec![10, 20, 30])));
        let (method, arg, call) = decode_rpc_request(&rpc).unwrap();
        assert_eq!(method, "echo");
        assert_eq!(call, CallSpec::default());
        assert_eq!(Bytes::from_ipc(&arg).unwrap().0, vec![10, 20, 30]);

        let resp_ok = encode_rpc_response(Ok(Bytes(vec![1, 2]).into_ipc()));
        assert_eq!(resp_ok[0], 0x00);
        assert_eq!(
            decode_wire_value(&resp_ok[1..]).unwrap(),
            Value::Bytes(vec![1, 2])
        );

        let resp_err = encode_rpc_response(Err("boom".into()));
        assert_eq!(resp_err[0], 0x01);
        assert_eq!(&resp_err[1..], b"boom");

        let mut send_evt = vec![SEND_OP_EVENT];
        send_evt.extend_from_slice(&4u16.to_le_bytes());
        send_evt.extend_from_slice(b"ping");
        send_evt.extend_from_slice(&encode_wire_value(&Value::Bytes(vec![42])));
        assert_eq!(
            decode_send_packet(&send_evt).unwrap(),
            ClientSendPacket::Event("ping".into(), Value::Bytes(vec![42]))
        );
        assert_eq!(
            decode_send_packet(&[SEND_OP_DRAG]).unwrap(),
            ClientSendPacket::Drag
        );

        let f1 = encode_event_frame("frame", &Value::Bytes(vec![1, 2, 3]));
        let batch = encode_event_batch(std::slice::from_ref(&f1));
        assert_eq!(&batch[0..4], &1u32.to_le_bytes());
        assert_eq!(&batch[4..], f1.as_slice());

        for sample in [b"".as_slice(), b"a", b"ab", b"abc", b"hello\x00world\xff"] {
            let b64 = encode_base64(sample);
            assert_eq!(decode_base64(&b64).unwrap(), sample);
        }
        assert!(decode_base64("abc").is_err());
        assert!(decode_base64("===a").is_err());
    }
}
