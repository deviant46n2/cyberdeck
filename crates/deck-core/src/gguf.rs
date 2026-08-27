//! Minimal GGUF header/metadata reader.
//!
//! Parses only the header + key/value metadata block (never tensor data),
//! which is what inventory scanning needs. Large arrays are NOT materialized;
//! their element type and count are recorded and the reader seeks past them.
//!
//! Format reference: https://github.com/ggml-org/ggml/blob/master/docs/gguf.md

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use thiserror::Error;

const MAGIC: u32 = 0x4655_4747;
const MAX_STRING_LEN: u64 = 1 << 24;

#[derive(Debug, Error)]
pub enum GgufError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("not a GGUF file (bad magic 0x{got:08x})")]
    BadMagic { got: u32 },
    #[error("GGUF version {version} unsupported")]
    BadVersion { version: u32 },
    #[error("file truncated while reading {where_}")]
    Truncated { where_: &'static str },
    #[error("unknown GGUF value type id {id}")]
    UnknownValueType { id: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueType {
    U8,
    I8,
    U16,
    I16,
    U32,
    I32,
    F32,
    Bool,
    String,
    Array,
    U64,
    I64,
    F64,
}

impl ValueType {
    fn from_id(id: u32) -> Result<Self, GgufError> {
        Ok(match id {
            0 => Self::U8,
            1 => Self::I8,
            2 => Self::U16,
            3 => Self::I16,
            4 => Self::U32,
            5 => Self::I32,
            6 => Self::F32,
            7 => Self::Bool,
            8 => Self::String,
            9 => Self::Array,
            10 => Self::U64,
            11 => Self::I64,
            12 => Self::F64,
            _ => return Err(GgufError::UnknownValueType { id }),
        })
    }

    fn byte_size(self) -> Option<u64> {
        match self {
            Self::U8 | Self::I8 | Self::Bool => Some(1),
            Self::U16 | Self::I16 => Some(2),
            Self::U32 | Self::I32 | Self::F32 => Some(4),
            Self::U64 | Self::I64 | Self::F64 => Some(8),
            Self::String | Self::Array => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
    Array { elem_type: ValueType, count: u64 },
}

impl Value {
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Value::Int(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Int(_) => "int",
            Value::Float(_) => "float",
            Value::Bool(_) => "bool",
            Value::Str(_) => "string",
            Value::Array { .. } => "array",
        }
    }
}

#[derive(Debug)]
pub struct GgufMeta {
    pub version: u32,
    pub tensor_count: u64,
    pub file_size: u64,
    pub kv: BTreeMap<String, Value>,
    /// True if parsing stopped early due to a truncated read (e.g. HTTP Range
    /// response that covers scalar KVs but not the full tokenizer array).
    pub truncated: bool,
}

impl GgufMeta {
    /// Reads header + metadata KVs. Cheap even on huge models: tensor data
    /// is never touched, oversized arrays are seeked past.
    pub fn read(path: impl AsRef<Path>) -> Result<Self, GgufError> {
        let file = File::open(path.as_ref())?;
        let file_size = file.metadata()?.len();
        let mut r = BufReader::new(file);
        let mut meta = parse(&mut r)?;
        meta.file_size = file_size;
        Ok(meta)
    }

    /// Parse from an in-memory reader (e.g. a Range-fetched GGUF header).
    /// `total_size` is the full file size on the remote server. Parsing is
    /// best-effort: all scalar KVs are read, but a large tokenizer array may
    /// exceed the buffer — in that case `truncated` is set and the remaining
    /// KVs are skipped. The critical fit fields (arch, block_count,
    /// embedding_length, file_type) are always scalars and will be present.
    pub fn from_reader(mut reader: impl Read + Seek, total_size: u64) -> Result<Self, GgufError> {
        let mut meta = parse(&mut reader)?;
        meta.file_size = total_size;
        Ok(meta)
    }

    pub fn arch(&self) -> Option<&str> {
        self.kv.get("general.architecture").and_then(Value::as_str)
    }

    pub fn name(&self) -> Option<&str> {
        self.kv.get("general.name").and_then(Value::as_str)
    }

    pub fn quant_name(&self) -> Option<String> {
        let code = self.kv.get("general.file_type")?.as_int()?;
        Some(file_type_name(code.clamp(0, u31_max()) as u32))
    }

    pub fn ctx_train(&self) -> Option<i64> {
        let arch = self.arch()?;
        self.kv.get(&format!("{arch}.context_length"))?.as_int()
    }

    /// Vocab size if present, via tokenizer array element count (array
    /// contents are never materialized).
    pub fn vocab_size(&self) -> Option<u64> {
        match self.kv.get("tokenizer.ggml.tokens")? {
            Value::Array { count, .. } => Some(*count),
            _ => None,
        }
    }

    pub fn n_layers(&self) -> Option<u64> {
        let arch = self.arch()?;
        self.kv
            .get(&format!("{arch}.block_count"))
            .and_then(Value::as_int)
            .map(|v| v as u64)
    }

    pub fn n_embd(&self) -> Option<u64> {
        let arch = self.arch()?;
        self.kv
            .get(&format!("{arch}.embedding_length"))
            .and_then(Value::as_int)
            .map(|v| v as u64)
    }

    pub fn params(&self) -> Option<u64> {
        self.kv
            .get("general.parameter_count")
            .and_then(Value::as_int)
            .map(|v| v as u64)
    }

    /// Convert into the unified model record.
    pub fn to_meta(&self, path: impl AsRef<Path>) -> crate::model::ModelMeta {
        use crate::model::{ModelFormat, ModelMeta};
        let weight_size = match self.params() {
            Some(p) => p / 2 * 2, // not used directly; file_size is ground truth
            None => 0,
        };
        let _ = weight_size;
        ModelMeta {
            path: path.as_ref().to_path_buf(),
            format: ModelFormat::Gguf,
            name: self.name().unwrap_or("unknown").to_string(),
            arch: self.arch().map(str::to_string),
            quant: self.quant_name(),
            params: self.params(),
            n_layers: self.n_layers(),
            n_embd: self.n_embd(),
            ctx_train: self.ctx_train().map(|v| v as u64),
            vocab: self.vocab_size(),
            weight_size: self.file_size,
            footprint: self.file_size,
        }
    }
}

fn u31_max() -> i64 {
    i32::MAX as i64
}

fn parse<R: Read + Seek>(r: &mut R) -> Result<GgufMeta, GgufError> {
    let mut b4 = [0u8; 4];
    let mut b8 = [0u8; 8];

    r.read_exact(&mut b4)
        .map_err(|_| GgufError::Truncated { where_: "magic" })?;
    let magic = u32::from_le_bytes(b4);
    if magic != MAGIC {
        return Err(GgufError::BadMagic { got: magic });
    }

    r.read_exact(&mut b4)
        .map_err(|_| GgufError::Truncated { where_: "version" })?;
    let version = u32::from_le_bytes(b4);
    match version {
        1..=3 => {}
        other => return Err(GgufError::BadVersion { version: other }),
    }

    // GGUF v1 used 32-bit counts and string lengths; v2+ use 64-bit.
    let legacy = version == 1;

    let tensor_count = if legacy {
        r.read_exact(&mut b4).map_err(trunc("tensor_count"))?;
        u32::from_le_bytes(b4) as u64
    } else {
        r.read_exact(&mut b8).map_err(trunc("tensor_count"))?;
        u64::from_le_bytes(b8)
    };

    let kv_count = if legacy {
        r.read_exact(&mut b4).map_err(trunc("kv_count"))?;
        u32::from_le_bytes(b4) as u64
    } else {
        r.read_exact(&mut b8).map_err(trunc("kv_count"))?;
        u64::from_le_bytes(b8)
    };

    let mut kv = BTreeMap::new();
    let mut truncated = false;
    for _ in 0..kv_count {
        let key = match read_string(r, legacy, &mut b4, &mut b8) {
            Ok(k) => k,
            Err(_) => {
                truncated = true;
                break;
            }
        };
        r.read_exact(&mut b4).map_err(trunc("value_type"))?;
        let vt = ValueType::from_id(u32::from_le_bytes(b4))?;
        let val = match read_value(r, vt, legacy, &mut b4, &mut b8) {
            Ok(v) => v,
            Err(_) => {
                truncated = true;
                break;
            }
        };
        kv.insert(key, val);
    }

    Ok(GgufMeta {
        version,
        tensor_count,
        file_size: 0,
        kv,
        truncated,
    })
}

fn trunc(where_: &'static str) -> impl Fn(std::io::Error) -> GgufError {
    move |_| GgufError::Truncated { where_ }
}

fn read_string<R: Read + Seek>(
    r: &mut R,
    legacy: bool,
    b4: &mut [u8; 4],
    b8: &mut [u8; 8],
) -> Result<String, GgufError> {
    let len = if legacy {
        r.read_exact(b4).map_err(trunc("string_len"))?;
        u32::from_le_bytes(*b4) as u64
    } else {
        r.read_exact(b8).map_err(trunc("string_len"))?;
        u64::from_le_bytes(*b8)
    };
    if len > MAX_STRING_LEN {
        return Err(GgufError::Truncated {
            where_: "string (implausible length)",
        });
    }
    let mut bytes = vec![0u8; len as usize];
    r.read_exact(&mut bytes).map_err(trunc("string_bytes"))?;
    String::from_utf8(bytes).map_err(|_| GgufError::Truncated {
        where_: "string utf8",
    })
}

fn read_value<R: Read + Seek>(
    r: &mut R,
    vt: ValueType,
    legacy_in_array: bool,
    b4: &mut [u8; 4],
    b8: &mut [u8; 8],
) -> Result<Value, GgufError> {
    use ValueType::*;
    match vt {
        U8 => {
            r.read_exact(&mut b8[..1]).map_err(trunc("u8"))?;
            Ok(Value::Int(b8[0] as i64))
        }
        I8 => {
            r.read_exact(&mut b8[..1]).map_err(trunc("i8"))?;
            Ok(Value::Int(b8[0] as i8 as i64))
        }
        Bool => {
            r.read_exact(&mut b8[..1]).map_err(trunc("bool"))?;
            Ok(Value::Bool(b8[0] != 0))
        }
        U16 => {
            r.read_exact(&mut b8[..2]).map_err(trunc("u16"))?;
            Ok(Value::Int(u16::from_le_bytes([b8[0], b8[1]]) as i64))
        }
        I16 => {
            r.read_exact(&mut b8[..2]).map_err(trunc("i16"))?;
            Ok(Value::Int(i16::from_le_bytes([b8[0], b8[1]]) as i64))
        }
        U32 => {
            r.read_exact(b4).map_err(trunc("u32"))?;
            Ok(Value::Int(u32::from_le_bytes(*b4) as i64))
        }
        I32 => {
            r.read_exact(b4).map_err(trunc("i32"))?;
            Ok(Value::Int(i32::from_le_bytes(*b4) as i64))
        }
        F32 => {
            r.read_exact(b4).map_err(trunc("f32"))?;
            Ok(Value::Float(f32::from_le_bytes(*b4) as f64))
        }
        U64 | I64 => {
            r.read_exact(b8).map_err(trunc("64-bit int"))?;
            Ok(Value::Int(i64::from_le_bytes(*b8)))
        }
        F64 => {
            r.read_exact(b8).map_err(trunc("f64"))?;
            Ok(Value::Float(f64::from_le_bytes(*b8)))
        }
        String => Ok(Value::Str(read_string(r, legacy_in_array, b4, b8)?)),
        Array => {
            r.read_exact(b4).map_err(trunc("array elem_type"))?;
            let elem_type = ValueType::from_id(u32::from_le_bytes(*b4))?;
            r.read_exact(b8).map_err(trunc("array count"))?;
            let count = u64::from_le_bytes(*b8);
            skip_array(r, elem_type, count, legacy_in_array, b4, b8)?;
            Ok(Value::Array { elem_type, count })
        }
    }
}

/// Advances past `count` elements of `elem`, keeping only their footprint.
fn skip_array<R: Read + Seek>(
    r: &mut R,
    elem: ValueType,
    count: u64,
    legacy: bool,
    b4: &mut [u8; 4],
    b8: &mut [u8; 8],
) -> Result<(), GgufError> {
    if let Some(stride) = elem.byte_size() {
        let total = stride.saturating_mul(count);
        let target = r
            .stream_position()?
            .checked_add(total)
            .ok_or(GgufError::Truncated {
                where_: "array seek",
            })?;
        // Seeking past EOF is fine here: no strict reads follow the last KV.
        r.seek(SeekFrom::Start(target)).or_else(|_| {
            std::io::copy(&mut r.take(total), &mut std::io::sink()).map_err(trunc("array skip"))
        })?;
        return Ok(());
    }

    match elem {
        ValueType::String => {
            for _ in 0..count {
                read_string(r, legacy, b4, b8)?;
            }
        }
        ValueType::Array => {
            for _ in 0..count {
                r.read_exact(b4).map_err(trunc("nested array type"))?;
                let inner = ValueType::from_id(u32::from_le_bytes(*b4))?;
                r.read_exact(b8).map_err(trunc("nested array count"))?;
                let inner_count = u64::from_le_bytes(*b8);
                skip_array(r, inner, inner_count, legacy, b4, b8)?;
            }
        }
        other => {
            let stride = other.byte_size().unwrap_or(0);
            std::io::copy(
                &mut r.take(stride.saturating_mul(count)),
                &mut std::io::sink(),
            )
            .map_err(trunc("array fallback"))?;
        }
    }
    Ok(())
}

/// Canonical quantization names for GGUF `general.file_type` codes.
/// Codes not yet in this table map to `unknown(code)`; filename heuristics
/// cover exotic dynamics later.
pub fn file_type_name(code: u32) -> String {
    let name = match code {
        0 => "F32",
        1 => "F16",
        2 => "Q4_0",
        3 => "Q4_1",
        7 => "Q8_0",
        8 => "Q5_0",
        9 => "Q5_1",
        10 => "Q2_K",
        11 => "Q3_K_S",
        12 => "Q3_K_M",
        13 => "Q3_K_L",
        14 => "Q4_K_S",
        15 => "Q4_K_M",
        16 => "Q5_K_S",
        17 => "Q5_K_M",
        18 => "Q6_K",
        19 => "IQ2_XXS",
        20 => "IQ2_XS",
        21 => "Q2_K_S",
        22 => "IQ3_XS",
        23 => "IQ3_XXS",
        24 => "IQ1_S",
        25 => "IQ4_NL",
        26 => "IQ3_S",
        27 => "IQ3_M",
        28 => "IQ2_S",
        29 => "IQ2_M",
        30 => "IQ4_XS",
        31 => "IQ1_M",
        32 => "BF16",
        36 => "TQ1_0",
        37 => "TQ2_0",
        38 => "MXFP4",
        _ => return format!("unknown({code})"),
    };
    name.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    struct Buf(Vec<u8>);

    impl Buf {
        fn u32(self, x: u32) -> Self {
            let mut b = self.0;
            b.extend_from_slice(&x.to_le_bytes());
            Self(b)
        }
        fn u64(self, x: u64) -> Self {
            let mut b = self.0;
            b.extend_from_slice(&x.to_le_bytes());
            Self(b)
        }
        fn str(self, s: &str) -> Self {
            self.u64(s.len() as u64).append(s.as_bytes())
        }
        fn append(self, bytes: &[u8]) -> Self {
            let mut b = self.0;
            b.extend_from_slice(bytes);
            Self(b)
        }
        fn done(self) -> Vec<u8> {
            self.0
        }
    }

    fn build_test_gguf(version: u32) -> Vec<u8> {
        Buf(vec![])
            .u32(MAGIC)
            .u32(version)
            .u64(0)
            .u64(6)
            .str("general.architecture")
            .u32(ValueType::String as u32)
            .str("qwen3")
            .str("general.file_type")
            .u32(ValueType::U32 as u32)
            .u32(15)
            .str("qwen3.context_length")
            .u32(ValueType::U32 as u32)
            .u32(262144)
            .str("test.flag")
            .u32(ValueType::Bool as u32)
            .append(&[1])
            .str("test.f")
            .u32(ValueType::F32 as u32)
            .append(&0.5f32.to_le_bytes())
            .str("tokenizer.ggml.tokens")
            .u32(ValueType::Array as u32)
            .u32(ValueType::String as u32)
            .u64(3)
            .str("<pad>")
            .str("hello")
            .str("world")
            .done()
    }

    #[test]
    fn parses_synthetic_v3() {
        let dir = tempdir("synthetic-v3");
        let path = dir.join("mini.gguf");
        std::fs::write(&path, build_test_gguf(3)).unwrap();

        let meta = GgufMeta::read(&path).unwrap();
        assert_eq!(meta.version, 3);
        assert_eq!(meta.arch(), Some("qwen3"));
        assert_eq!(meta.quant_name().as_deref(), Some("Q4_K_M"));
        assert_eq!(meta.ctx_train(), Some(262144));
        assert_eq!(meta.vocab_size(), Some(3));
        assert!(
            matches!(meta.kv.get("test.flag"), Some(Value::Bool(true))),
            "bool KV should round-trip"
        );
        assert!(
            matches!(
                meta.kv.get("tokenizer.ggml.tokens"),
                Some(Value::Array {
                    elem_type: ValueType::String,
                    count: 3
                })
            ),
            "token array should be skipped but counted"
        );
        cleanup(&dir);
    }

    #[test]
    fn seeks_past_large_fixed_stride_array() {
        let dir = tempdir("bigarray");
        let path = dir.join("bigarray.gguf");
        let mut bytes = Buf(vec![])
            .u32(MAGIC)
            .u32(3)
            .u64(0)
            .u64(1)
            .str("embeddings.dummy")
            .u32(ValueType::Array as u32)
            .u32(ValueType::F32 as u32)
            .u64(2_000_000)
            .done();
        bytes.extend(std::iter::repeat_n([0u8; 4], 2_000_000).flatten());
        std::fs::write(&path, bytes).unwrap();

        let meta = GgufMeta::read(&path).unwrap();
        assert_eq!(meta.file_size, path.metadata().unwrap().len());
        assert_eq!(
            meta.kv.get("embeddings.dummy"),
            Some(&Value::Array {
                elem_type: ValueType::F32,
                count: 2_000_000
            })
        );
        cleanup(&dir);
    }

    #[test]
    fn rejects_bad_magic() {
        let dir = tempdir("badmagic");
        let path = dir.join("bad.gguf");
        std::fs::write(&path, b"NOTGGUFF NOT").unwrap();
        let err = GgufMeta::read(&path).unwrap_err();
        assert!(matches!(err, GgufError::BadMagic { got: _ }));
        cleanup(&dir);
    }

    #[test]
    fn rejects_future_version() {
        let dir = tempdir("futurever");
        let path = dir.join("future.gguf");
        let bytes = Buf(vec![]).u32(MAGIC).u32(99).u64(0).u64(0).done();
        std::fs::write(&path, bytes).unwrap();
        let err = GgufMeta::read(&path).unwrap_err();
        assert!(matches!(err, GgufError::BadVersion { version: 99 }));
        cleanup(&dir);
    }

    #[test]
    fn reads_real_fixture_if_present() {
        let candidates = [
            "~/Clone/llama.cpp/models/ggml-vocab-qwen2.gguf",
            "../../Clone/llama.cpp/models/ggml-vocab-qwen2.gguf",
        ];
        let found = candidates
            .iter()
            .map(|p| shellexpand_home(p))
            .map(std::path::PathBuf::from)
            .find(|p| p.exists());
        let Some(fixture) = found else {
            eprintln!("fixture not present locally, skipping real-file test");
            return;
        };
        let meta = GgufMeta::read(&fixture).unwrap();
        assert!(meta.arch().is_some(), "real fixture should expose arch");
        assert_eq!(meta.file_size, fixture.metadata().unwrap().len());
    }

    #[test]
    fn from_reader_parses_clean_buffer() {
        let bytes = build_test_gguf(3);
        let total = bytes.len() as u64 * 3; // pretend full file is larger
        let mut cursor = Cursor::new(bytes);
        let meta = GgufMeta::from_reader(&mut cursor, total).unwrap();
        assert_eq!(meta.arch(), Some("qwen3"));
        assert_eq!(meta.quant_name().as_deref(), Some("Q4_K_M"));
        assert_eq!(meta.n_layers(), None); // not in test fixture
        assert_eq!(meta.file_size, total);
        assert!(!meta.truncated, "no truncation when all data fits");
    }

    #[test]
    fn from_reader_tolerates_truncated_array() {
        // Build a GGUF with 6 KVs, then truncate after the scalar KVs
        // (before the tokenizer array data), simulating a Range header fetch.
        let full = build_test_gguf(3);
        // Find the start of the tokenizer array: after test.f + F32 value.
        // The F32 value for "test.f" is 4 bytes. Everything after is the
        // tokenizer array key + type + array header + data.
        // We want to cut right at the array element_type byte (start of array).
        // Simpler: cut at 70% of the buffer — well past the scalars, mid-array.
        let cut = (full.len() as f64 * 0.7) as usize;
        let truncated_buf: Vec<u8> = full[..cut].to_vec();
        let total = full.len() as u64;
        let mut cursor = Cursor::new(truncated_buf);
        let meta = GgufMeta::from_reader(&mut cursor, total).unwrap();
        assert!(meta.truncated, "should detect truncation");
        assert_eq!(meta.arch(), Some("qwen3"));
        assert_eq!(
            meta.kv.get("general.file_type").and_then(Value::as_int),
            Some(15)
        );
        assert_eq!(meta.ctx_train(), Some(262144));
    }

    fn shellexpand_home(p: &str) -> String {
        if let Some(rest) = p.strip_prefix("~/") {
            if let Some(home) = std::env::var_os("HOME") {
                return std::path::Path::new(&home).join(rest).display().to_string();
            }
        }
        p.to_string()
    }

    fn tempdir(label: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("deck-core-test-{}-{label}", std::process::id()));
        let _ = std::fs::create_dir_all(&d);
        d
    }

    fn cleanup(d: &std::path::Path) {
        let _ = std::fs::remove_dir_all(d);
    }
}
