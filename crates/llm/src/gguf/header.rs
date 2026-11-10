use anyhow::{anyhow, Result};
use byteorder::{LittleEndian, ReadBytesExt};
use std::collections::HashMap;
use std::io::Cursor;

pub const GGUF_MAGIC: u32 = 0x46554747;

#[derive(Debug, Clone)]
pub enum GgufValue {
    Uint8(u8),
    Int8(i8),
    Uint16(u16),
    Int16(i16),
    Uint32(u32),
    Int32(i32),
    Float32(f32),
    Bool(bool),
    String(String),
    Array(Vec<GgufValue>),
    Uint64(u64),
    Int64(i64),
    Float64(f64),
}

impl GgufValue {
    pub fn to_u64(&self) -> Option<u64> {
        match self {
            GgufValue::Uint8(v) => Some(*v as u64),
            GgufValue::Int8(v) => Some(*v as u64),
            GgufValue::Uint16(v) => Some(*v as u64),
            GgufValue::Int16(v) => Some(*v as u64),
            GgufValue::Uint32(v) => Some(*v as u64),
            GgufValue::Int32(v) => Some(*v as u64),
            GgufValue::Uint64(v) => Some(*v),
            GgufValue::Int64(v) => Some(*v as u64),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TensorInfo {
    pub name: String,
    pub n_dimensions: u32,
    pub dimensions: Vec<u64>,
    pub qtype: u32,
    pub offset: u64,
}

pub struct GgufHeader {
    pub version: u32,
    pub tensor_count: u64,
    pub metadata_count: u64,
    pub metadata: HashMap<String, GgufValue>,
    pub tensors: Vec<TensorInfo>,
}

impl GgufHeader {
    pub fn parse(reader: &mut Cursor<&[u8]>) -> Result<Self> {
        let magic = reader.read_u32::<LittleEndian>()?;
        if magic != GGUF_MAGIC {
            return Err(anyhow!("Geçersiz GGUF Magic Number: 0x{:X}", magic));
        }

        let version = reader.read_u32::<LittleEndian>()?;
        let tensor_count = reader.read_u64::<LittleEndian>()?;
        let metadata_count = reader.read_u64::<LittleEndian>()?;

        let mut metadata = HashMap::new();
        for _ in 0..metadata_count {
            let key = Self::read_string(reader)?;
            let val_type = reader.read_u32::<LittleEndian>()?;
            let val = Self::read_value(reader, val_type)?;
            metadata.insert(key, val);
        }

        let mut tensors = Vec::with_capacity(tensor_count as usize);
        for _ in 0..tensor_count {
            let name = Self::read_string(reader)?;
            let n_dimensions = reader.read_u32::<LittleEndian>()?;
            let mut dimensions = Vec::with_capacity(n_dimensions as usize);
            for _ in 0..n_dimensions {
                dimensions.push(reader.read_u64::<LittleEndian>()?);
            }
            let qtype = reader.read_u32::<LittleEndian>()?;
            let offset = reader.read_u64::<LittleEndian>()?;

            tensors.push(TensorInfo {
                name,
                n_dimensions,
                dimensions,
                qtype,
                offset,
            });
        }

        Ok(Self {
            version,
            tensor_count,
            metadata_count,
            metadata,
            tensors,
        })
    }

    fn read_string(reader: &mut Cursor<&[u8]>) -> Result<String> {
        let len = reader.read_u64::<LittleEndian>()? as usize;
        let mut buf = vec![0u8; len];
        use std::io::Read;
        reader.read_exact(&mut buf)?;
        Ok(String::from_utf8_lossy(&buf).to_string())
    }

    fn read_value(reader: &mut Cursor<&[u8]>, val_type: u32) -> Result<GgufValue> {
        match val_type {
            0 => Ok(GgufValue::Uint8(reader.read_u8()?)),
            1 => Ok(GgufValue::Int8(reader.read_i8()?)),
            2 => Ok(GgufValue::Uint16(reader.read_u16::<LittleEndian>()?)),
            3 => Ok(GgufValue::Int16(reader.read_i16::<LittleEndian>()?)),
            4 => Ok(GgufValue::Uint32(reader.read_u32::<LittleEndian>()?)),
            5 => Ok(GgufValue::Int32(reader.read_i32::<LittleEndian>()?)),
            6 => Ok(GgufValue::Float32(reader.read_f32::<LittleEndian>()?)),
            7 => Ok(GgufValue::Bool(reader.read_u8()? != 0)),
            8 => Ok(GgufValue::String(Self::read_string(reader)?)),
            9 => {
                let elem_type = reader.read_u32::<LittleEndian>()?;
                let len = reader.read_u64::<LittleEndian>()? as usize;
                let mut arr = Vec::with_capacity(len);
                for _ in 0..len {
                    arr.push(Self::read_value(reader, elem_type)?);
                }
                Ok(GgufValue::Array(arr))
            }
            10 => Ok(GgufValue::Uint64(reader.read_u64::<LittleEndian>()?)),
            11 => Ok(GgufValue::Int64(reader.read_i64::<LittleEndian>()?)),
            12 => Ok(GgufValue::Float64(reader.read_f64::<LittleEndian>()?)),
            _ => Err(anyhow!("Bilinmeyen GGUF Veri Tipi: {}", val_type)),
        }
    }
}
