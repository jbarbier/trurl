//! A small, defensive GGUF header reader.
//!
//! Trurl reads the header of every model file to show a friendly name, to know
//! the model's trained context length and KV shape (for the RAM estimate), to
//! tell whether it has a chat template, and to catch truncated copies before
//! llama-server tries to load them. Only the metadata and tensor table are read;
//! the weights are never touched. Every length is bounded so a damaged file
//! cannot make Trurl allocate gigabytes.

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

const MAGIC: &[u8; 4] = b"GGUF";
const MAX_STRING: u64 = 16 * 1024 * 1024; // chat templates are a few KB; 16 MB is generous
const MAX_KV: u64 = 100_000;
/// Keys are names like `general.architecture`: a few dozen bytes.
const MAX_KEY: u64 = 64 * 1024;
/// Strings Trurl keeps (`general.*`, the chat template), all together. A file
/// that asks for more is not a model worth trusting with the learner's memory:
/// the rest is skipped, not read.
const MAX_KEPT_TOTAL: u64 = 4 * 1024 * 1024;
const MAX_TENSORS: u64 = 1_000_000;
const MAX_DIMS: u32 = 8;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct GgufInfo {
    pub version: u32,
    pub architecture: Option<String>,
    pub name: Option<String>,
    pub size_label: Option<String>,
    pub context_length: Option<u64>,
    pub block_count: Option<u64>,
    pub embedding_length: Option<u64>,
    pub head_count: Option<u64>,
    pub head_count_kv: Option<u64>,
    pub key_length: Option<u64>,
    pub value_length: Option<u64>,
    pub file_type: Option<u64>,
    pub split_count: Option<u64>,
    pub has_chat_template: bool,
    /// The chat template mentions thinking tags: a hint that this is a reasoning model.
    pub template_mentions_thinking: bool,
    /// Bytes the tensor table says the file must at least contain.
    pub min_file_size: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum GgufError {
    NotGguf,
    Truncated,
    Malformed(String),
    Io(String),
}

impl std::fmt::Display for GgufError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GgufError::NotGguf => write!(f, "not a GGUF file"),
            GgufError::Truncated => write!(f, "file is truncated"),
            GgufError::Malformed(m) => write!(f, "malformed GGUF: {m}"),
            GgufError::Io(m) => write!(f, "{m}"),
        }
    }
}

impl From<std::io::Error> for GgufError {
    fn from(e: std::io::Error) -> Self {
        if e.kind() == std::io::ErrorKind::UnexpectedEof {
            GgufError::Truncated
        } else {
            GgufError::Io(e.to_string())
        }
    }
}

type R<T> = std::result::Result<T, GgufError>;

#[derive(Debug, Clone)]
enum Value {
    U(u64),
    I(i64),
    F(f64),
    Str(String),
    Other,
}

struct Reader<T: Read + Seek> {
    r: T,
    /// Bytes of strings kept so far (see MAX_KEPT_TOTAL).
    kept: u64,
}

impl<T: Read + Seek> Reader<T> {
    fn u8(&mut self) -> R<u8> {
        let mut b = [0u8; 1];
        self.r.read_exact(&mut b)?;
        Ok(b[0])
    }
    fn u32(&mut self) -> R<u32> {
        let mut b = [0u8; 4];
        self.r.read_exact(&mut b)?;
        Ok(u32::from_le_bytes(b))
    }
    fn u64(&mut self) -> R<u64> {
        let mut b = [0u8; 8];
        self.r.read_exact(&mut b)?;
        Ok(u64::from_le_bytes(b))
    }
    /// Skip `n` bytes. Small skips read through the buffer: seeking a BufReader
    /// throws its buffer away, and a vocabulary has hundreds of thousands of
    /// small strings to skip (one seek each would re-read 64 KB every time).
    fn skip(&mut self, n: u64) -> R<()> {
        if n <= 1 << 20 {
            let copied = std::io::copy(&mut (&mut self.r).take(n), &mut std::io::sink())?;
            if copied < n {
                return Err(GgufError::Truncated);
            }
            return Ok(());
        }
        let n = i64::try_from(n).map_err(|_| GgufError::Malformed("skip too large".into()))?;
        self.r.seek(SeekFrom::Current(n))?;
        Ok(())
    }
    fn key(&mut self) -> R<String> {
        let n = self.u64()?;
        if n > MAX_KEY {
            return Err(GgufError::Malformed(format!("key of {n} bytes")));
        }
        let mut b = vec![0u8; n as usize];
        self.r.read_exact(&mut b)?;
        Ok(String::from_utf8_lossy(&b).into_owned())
    }
    fn skip_string(&mut self) -> R<()> {
        let n = self.u64()?;
        if n > MAX_STRING {
            return Err(GgufError::Malformed(format!("string of {n} bytes")));
        }
        self.skip(n)
    }
    fn value(&mut self, ty: u32, keep_string: bool) -> R<Value> {
        Ok(match ty {
            0 => Value::U(self.u8()? as u64),
            1 => Value::I(self.u8()? as i8 as i64),
            2 => {
                let mut b = [0u8; 2];
                self.r.read_exact(&mut b)?;
                Value::U(u16::from_le_bytes(b) as u64)
            }
            3 => {
                let mut b = [0u8; 2];
                self.r.read_exact(&mut b)?;
                Value::I(i16::from_le_bytes(b) as i64)
            }
            4 => Value::U(self.u32()? as u64),
            5 => Value::I(self.u32()? as i32 as i64),
            6 => Value::F(f32::from_bits(self.u32()?) as f64),
            7 => Value::U((self.u8()? != 0) as u64),
            8 => {
                let n = self.u64()?;
                if n > MAX_STRING {
                    return Err(GgufError::Malformed(format!("string of {n} bytes")));
                }
                if keep_string && self.kept + n <= MAX_KEPT_TOTAL {
                    self.kept += n;
                    let mut b = vec![0u8; n as usize];
                    self.r.read_exact(&mut b)?;
                    Value::Str(String::from_utf8_lossy(&b).into_owned())
                } else {
                    self.skip(n)?;
                    Value::Other
                }
            }
            9 => {
                let item_ty = self.u32()?;
                let n = self.u64()?;
                match item_ty {
                    0 | 1 | 7 => self.skip(n)?,
                    2 | 3 => self.skip(
                        n.checked_mul(2)
                            .ok_or(GgufError::Malformed("array".into()))?,
                    )?,
                    4..=6 => self.skip(
                        n.checked_mul(4)
                            .ok_or(GgufError::Malformed("array".into()))?,
                    )?,
                    10..=12 => self.skip(
                        n.checked_mul(8)
                            .ok_or(GgufError::Malformed("array".into()))?,
                    )?,
                    8 => {
                        for _ in 0..n {
                            self.skip_string()?;
                        }
                    }
                    _ => return Err(GgufError::Malformed(format!("array of type {item_ty}"))),
                }
                Value::Other
            }
            10 => Value::U(self.u64()?),
            11 => Value::I(self.u64()? as i64),
            12 => Value::F(f64::from_bits(self.u64()?)),
            _ => return Err(GgufError::Malformed(format!("value type {ty}"))),
        })
    }
}

fn as_u64(v: &Value) -> Option<u64> {
    match v {
        Value::U(u) => Some(*u),
        Value::I(i) if *i >= 0 => Some(*i as u64),
        Value::F(f) if *f >= 0.0 => Some(*f as u64),
        Value::Str(_) | Value::Other | Value::I(_) | Value::F(_) => None,
    }
}

pub fn read(path: &Path) -> R<GgufInfo> {
    read_from(BufReader::with_capacity(64 * 1024, File::open(path)?))
}

pub fn read_from<T: Read + Seek>(r: T) -> R<GgufInfo> {
    let mut rd = Reader { r, kept: 0 };
    let mut magic = [0u8; 4];
    match rd.r.read_exact(&mut magic) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Err(GgufError::NotGguf),
        Err(e) => return Err(e.into()),
    }
    if &magic != MAGIC {
        return Err(GgufError::NotGguf);
    }
    let version = rd.u32()?;
    if !(2..=3).contains(&version) {
        return Err(GgufError::Malformed(format!("version {version}")));
    }
    let n_tensors = rd.u64()?;
    let n_kv = rd.u64()?;
    if n_kv > MAX_KV || n_tensors > MAX_TENSORS {
        return Err(GgufError::Malformed("counts out of range".into()));
    }

    let mut info = GgufInfo {
        version,
        ..Default::default()
    };
    let mut kv: Vec<(String, Value)> = Vec::new();
    let mut alignment: u64 = 32;
    for _ in 0..n_kv {
        let key = rd.key()?;
        let ty = rd.u32()?;
        let keep = key.starts_with("general.") || key == "tokenizer.chat_template";
        let v = rd.value(ty, keep)?;
        if key == "general.alignment" {
            alignment = as_u64(&v)
                .filter(|a| *a > 0 && a.is_power_of_two())
                .unwrap_or(32);
        }
        kv.push((key, v));
    }

    let get_str = |k: &str| {
        kv.iter()
            .find(|(kk, _)| kk == k)
            .and_then(|(_, v)| match v {
                Value::Str(s) => Some(s.clone()),
                _ => None,
            })
    };
    info.architecture = get_str("general.architecture");
    info.name = get_str("general.name").filter(|s| !s.trim().is_empty());
    info.size_label = get_str("general.size_label");
    if let Some(t) = get_str("tokenizer.chat_template") {
        info.has_chat_template = !t.trim().is_empty();
        info.template_mentions_thinking = t.contains("<think>")
            || t.contains("reasoning_content")
            || t.contains("enable_thinking");
    }
    let num = |k: &str| {
        kv.iter()
            .find(|(kk, _)| kk == k)
            .and_then(|(_, v)| as_u64(v))
    };
    info.file_type = num("general.file_type");
    info.split_count = num("split.count");
    if let Some(arch) = info.architecture.clone() {
        info.context_length = num(&format!("{arch}.context_length"));
        info.block_count = num(&format!("{arch}.block_count"));
        info.embedding_length = num(&format!("{arch}.embedding_length"));
        info.head_count = num(&format!("{arch}.attention.head_count"));
        info.head_count_kv = num(&format!("{arch}.attention.head_count_kv")).or(info.head_count);
        info.key_length = num(&format!("{arch}.attention.key_length"));
        info.value_length = num(&format!("{arch}.attention.value_length"));
    }

    // Tensor table: find where the data section must end at least.
    let mut max_offset: u64 = 0;
    for _ in 0..n_tensors {
        rd.skip_string()?; // name
        let n_dims = rd.u32()?;
        if n_dims > MAX_DIMS {
            return Err(GgufError::Malformed(format!("{n_dims} dims")));
        }
        rd.skip(8 * n_dims as u64)?;
        let _ty = rd.u32()?;
        let offset = rd.u64()?;
        max_offset = max_offset.max(offset);
    }
    let header_end = rd.r.stream_position()?;
    let data_start = header_end.div_ceil(alignment) * alignment;
    // The last tensor starts at max_offset and holds at least one byte.
    // Hostile offsets near u64::MAX must not wrap around and pass the size check.
    info.min_file_size = if n_tensors > 0 {
        data_start
            .checked_add(max_offset)
            .and_then(|x| x.checked_add(1))
            .ok_or_else(|| GgufError::Malformed("tensor offset out of range".into()))?
    } else {
        header_end
    };
    let len = rd.r.seek(SeekFrom::End(0))?;
    if len < info.min_file_size {
        return Err(GgufError::Truncated);
    }
    Ok(info)
}

impl GgufInfo {
    /// Bytes of KV cache llama.cpp allocates for `ctx` tokens at f16.
    pub fn kv_cache_bytes(&self, ctx: u64) -> Option<u64> {
        let layers = self.block_count?;
        let heads_kv = self.head_count_kv?;
        let head_dim_default = self.embedding_length? / self.head_count.filter(|h| *h > 0)?;
        let k = self.key_length.unwrap_or(head_dim_default);
        let v = self.value_length.unwrap_or(head_dim_default);
        Some(ctx * layers * heads_kv * (k + v) * 2)
    }

    /// Is this file the first (or only) part of a model? Later shards are hidden.
    pub fn is_secondary_split(file_name: &str) -> bool {
        split_part(file_name).map(|(i, _)| i > 1).unwrap_or(false)
    }
}

/// `name-00002-of-00003.gguf` -> Some((2, 3)).
pub fn split_part(file_name: &str) -> Option<(u32, u32)> {
    let stem = file_name.strip_suffix(".gguf")?;
    let (head, total) = stem.rsplit_once("-of-")?;
    let (_, idx) = head.rsplit_once('-')?;
    if idx.len() != 5 || total.len() != 5 {
        return None;
    }
    Some((idx.parse().ok()?, total.parse().ok()?))
}

/// Build a GGUF file in memory. Used by tests here and in other modules.
#[doc(hidden)]
pub mod testfile {
    pub enum V<'a> {
        U32(u32),
        Str(&'a str),
        StrArray(&'a [&'a str]),
    }

    pub fn build(kv: &[(&str, V)], tensors: &[(&str, u64)], data_len: usize) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(b"GGUF");
        b.extend_from_slice(&3u32.to_le_bytes());
        b.extend_from_slice(&(tensors.len() as u64).to_le_bytes());
        b.extend_from_slice(&(kv.len() as u64).to_le_bytes());
        let s = |b: &mut Vec<u8>, t: &str| {
            b.extend_from_slice(&(t.len() as u64).to_le_bytes());
            b.extend_from_slice(t.as_bytes());
        };
        for (k, v) in kv {
            s(&mut b, k);
            match v {
                V::U32(x) => {
                    b.extend_from_slice(&4u32.to_le_bytes());
                    b.extend_from_slice(&x.to_le_bytes());
                }
                V::Str(t) => {
                    b.extend_from_slice(&8u32.to_le_bytes());
                    s(&mut b, t);
                }
                V::StrArray(items) => {
                    b.extend_from_slice(&9u32.to_le_bytes());
                    b.extend_from_slice(&8u32.to_le_bytes());
                    b.extend_from_slice(&(items.len() as u64).to_le_bytes());
                    for it in items.iter() {
                        s(&mut b, it);
                    }
                }
            }
        }
        for (name, off) in tensors {
            s(&mut b, name);
            b.extend_from_slice(&1u32.to_le_bytes());
            b.extend_from_slice(&16u64.to_le_bytes());
            b.extend_from_slice(&0u32.to_le_bytes());
            b.extend_from_slice(&off.to_le_bytes());
        }
        while b.len() % 32 != 0 {
            b.push(0);
        }
        b.extend(std::iter::repeat_n(0u8, data_len));
        b
    }

    /// A small but valid-looking chat model header.
    pub fn chat_model(name: &str) -> Vec<u8> {
        build(
            &[
                ("general.architecture", V::Str("llama")),
                ("general.name", V::Str(name)),
                ("llama.context_length", V::U32(4096)),
                ("llama.block_count", V::U32(2)),
                ("llama.embedding_length", V::U32(64)),
                ("llama.attention.head_count", V::U32(4)),
                ("llama.attention.head_count_kv", V::U32(2)),
                ("tokenizer.ggml.tokens", V::StrArray(&["a", "b", "c"])),
                (
                    "tokenizer.chat_template",
                    V::Str("{% for m in messages %}{{ m.content }}{% endfor %}"),
                ),
            ],
            &[("w", 0), ("x", 64)],
            128,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::testfile::*;
    use super::*;
    use std::io::Cursor;

    #[test]
    fn reads_the_fields_trurl_uses() {
        let b = build(
            &[
                ("general.architecture", V::Str("qwen3")),
                ("general.name", V::Str("Qwen3 0.6B")),
                ("general.size_label", V::Str("0.6B")),
                ("qwen3.context_length", V::U32(40960)),
                ("qwen3.block_count", V::U32(28)),
                ("qwen3.embedding_length", V::U32(1024)),
                ("qwen3.attention.head_count", V::U32(16)),
                ("qwen3.attention.head_count_kv", V::U32(8)),
                ("qwen3.attention.key_length", V::U32(128)),
                ("qwen3.attention.value_length", V::U32(128)),
                ("general.file_type", V::U32(15)),
                (
                    "tokenizer.ggml.tokens",
                    V::StrArray(&["<unk>", "hello", "world"]),
                ),
                (
                    "tokenizer.chat_template",
                    V::Str("{%- if enable_thinking %}<think>{% endif %}"),
                ),
            ],
            &[("a", 0), ("b", 1024)],
            2048,
        );
        let i = read_from(Cursor::new(&b)).unwrap();
        assert_eq!(i.architecture.as_deref(), Some("qwen3"));
        assert_eq!(i.name.as_deref(), Some("Qwen3 0.6B"));
        assert_eq!(i.context_length, Some(40960));
        assert_eq!(i.file_type, Some(15));
        assert!(i.has_chat_template && i.template_mentions_thinking);
        // 2048 ctx * 28 layers * 8 kv heads * (128+128) * 2 bytes = 224 MiB
        assert_eq!(i.kv_cache_bytes(2048), Some(2048 * 28 * 8 * 256 * 2));
        assert!(i.min_file_size > 1024);
    }

    #[test]
    fn base_model_without_template() {
        let b = build(&[("general.architecture", V::Str("gemma3"))], &[], 0);
        let i = read_from(Cursor::new(&b)).unwrap();
        assert!(!i.has_chat_template);
        assert_eq!(i.name, None);
    }

    #[test]
    fn rejects_non_gguf_and_empty() {
        assert_eq!(
            read_from(Cursor::new(b"PK\x03\x04zipzipzip".to_vec())),
            Err(GgufError::NotGguf)
        );
        assert_eq!(
            read_from(Cursor::new(Vec::<u8>::new())),
            Err(GgufError::NotGguf)
        );
    }

    #[test]
    fn every_truncation_of_the_header_is_caught() {
        let b = chat_model("M");
        // 128 data bytes, the last tensor starts at offset 64: any cut before it is truncation.
        for cut in 4..b.len() - 64 {
            let r = read_from(Cursor::new(&b[..cut]));
            assert!(r.is_err(), "cut at {cut} parsed");
        }
    }

    #[test]
    fn truncated_data_section_is_caught_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        let b = chat_model("M");
        let p = dir.path().join("m.gguf");
        std::fs::write(&p, &b).unwrap();
        assert!(read(&p).is_ok());
        std::fs::write(&p, &b[..b.len() - 100]).unwrap();
        assert_eq!(read(&p), Err(GgufError::Truncated));
    }

    #[test]
    fn absurd_lengths_do_not_allocate() {
        let mut b = Vec::new();
        b.extend_from_slice(b"GGUF");
        b.extend_from_slice(&3u32.to_le_bytes());
        b.extend_from_slice(&0u64.to_le_bytes());
        b.extend_from_slice(&1u64.to_le_bytes());
        b.extend_from_slice(&u64::MAX.to_le_bytes()); // key length
        assert!(matches!(
            read_from(Cursor::new(&b)),
            Err(GgufError::Malformed(_))
        ));
    }

    #[test]
    fn hostile_tensor_offset_does_not_wrap() {
        let b = build(
            &[("general.architecture", V::Str("llama"))],
            &[("w", u64::MAX)],
            64,
        );
        assert!(matches!(
            read_from(Cursor::new(&b)),
            Err(GgufError::Malformed(_))
        ));
        let b = build(
            &[("general.architecture", V::Str("llama"))],
            &[("w", u64::MAX - 100)],
            64,
        );
        assert!(read_from(Cursor::new(&b)).is_err());
    }

    #[test]
    fn split_file_names() {
        assert_eq!(split_part("model-00001-of-00003.gguf"), Some((1, 3)));
        assert!(GgufInfo::is_secondary_split("model-00002-of-00003.gguf"));
        assert!(!GgufInfo::is_secondary_split("model-00001-of-00003.gguf"));
        assert!(!GgufInfo::is_secondary_split("gemma-3-1b-it-Q4_K_M.gguf"));
        assert_eq!(split_part("x-1-of-2.gguf"), None);
    }
}
