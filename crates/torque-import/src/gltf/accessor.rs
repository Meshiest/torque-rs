//! Accessors read with every offset, stride and count checked against the buffer before an element is read.

use gltf::json;
use gltf::json::accessor::{ComponentType, Type};
use gltf::json::validation::Checked;

use crate::{Charge, ImportError, caps, pay};

/// An accessor's elements in place: `count` elements of `dims` components each, `stride` bytes apart from `start` in `bytes`
/// (no bytes: every element is zero).
pub(crate) struct Raw<'a> {
    bytes: &'a [u8],
    start: usize,
    stride: usize,
    pub count: usize,
    comp: ComponentType,
    pub dims: usize,
    normalized: bool,
}

fn malformed<T>(what: String) -> Result<T, ImportError> {
    Err(ImportError::Malformed(what))
}

fn comp_size(c: ComponentType) -> usize {
    match c {
        ComponentType::I8 | ComponentType::U8 => 1,
        ComponentType::I16 | ComponentType::U16 => 2,
        ComponentType::U32 | ComponentType::F32 => 4,
    }
}

/// Accessor `index` of the document over `buffers` (each the bytes of the buffer of that index), paid for by its element
/// count.
pub(crate) fn raw<'a>(
    root: &json::Root,
    buffers: &'a [Vec<u8>],
    index: usize,
    charge: &mut Charge,
) -> Result<Raw<'a>, ImportError> {
    let Some(a) = root.accessors.get(index) else {
        return malformed(format!("accessor {index}"));
    };
    if a.sparse.is_some() {
        return Err(ImportError::Unsupported(format!("sparse accessor {index}")));
    }
    let count = usize::try_from(a.count.0).unwrap_or(usize::MAX);
    crate::cap("accessor elements", count, caps::MAX_ACCESSOR_ELEMENTS)?;
    pay(charge, count as u64)?;
    let (Checked::Valid(comp), Checked::Valid(kind)) = (&a.component_type, &a.type_) else {
        return malformed(format!("accessor {index}'s type"));
    };
    let comp = comp.0;
    let dims = match kind {
        Type::Scalar => 1,
        Type::Vec2 => 2,
        Type::Vec3 => 3,
        Type::Vec4 => 4,
        Type::Mat4 => 16,
        // Matrices of 2 and 3 columns pad their columns; nothing a model needs uses them.
        Type::Mat2 | Type::Mat3 => {
            return Err(ImportError::Unsupported(format!(
                "accessor {index}'s matrix type"
            )));
        }
    };
    let elem = comp_size(comp) * dims;
    let Some(view_index) = a.buffer_view else {
        return Ok(Raw {
            bytes: &[],
            start: 0,
            stride: elem,
            count,
            comp,
            dims,
            normalized: a.normalized,
        });
    };
    let Some(view) = root.buffer_views.get(view_index.value()) else {
        return malformed(format!("accessor {index}'s view"));
    };
    let Some(buffer) = buffers.get(view.buffer.value()) else {
        return malformed(format!("accessor {index}'s buffer"));
    };
    let view_offset = usize::try_from(view.byte_offset.map_or(0, |o| o.0)).unwrap_or(usize::MAX);
    let view_length = usize::try_from(view.byte_length.0).unwrap_or(usize::MAX);
    let Some(view_bytes) = view_offset
        .checked_add(view_length)
        .and_then(|end| buffer.get(view_offset..end))
    else {
        return malformed(format!(
            "buffer view {} past its buffer",
            view_index.value()
        ));
    };
    let stride = view.byte_stride.map_or(elem, |s| s.0);
    if stride < elem || stride > 252 {
        return malformed(format!("accessor {index}'s stride {stride}"));
    }
    let start = usize::try_from(a.byte_offset.map_or(0, |o| o.0)).unwrap_or(usize::MAX);
    if count > 0 {
        let end = (count - 1)
            .checked_mul(stride)
            .and_then(|s| s.checked_add(start))
            .and_then(|s| s.checked_add(elem));
        if end.is_none_or(|e| e > view_bytes.len()) {
            return malformed(format!("accessor {index} past its buffer view"));
        }
    }
    Ok(Raw {
        bytes: view_bytes,
        start,
        stride,
        count,
        comp,
        dims,
        normalized: a.normalized,
    })
}

impl Raw<'_> {
    fn at(&self, e: usize, c: usize) -> usize {
        self.start + e * self.stride + c * comp_size(self.comp)
    }

    fn word(&self, at: usize, n: usize) -> u32 {
        let mut w = [0u8; 4];
        if let Some(b) = self.bytes.get(at..at + n) {
            w[..n].copy_from_slice(b);
        }
        u32::from_le_bytes(w)
    }

    /// Component `c` of element `e` as a number: a float as it is, a normalized integer as glTF maps it to -1..1 or 0..1, any
    /// other integer as its value.
    pub fn f(&self, e: usize, c: usize) -> f32 {
        if self.bytes.is_empty() {
            return 0.0;
        }
        let at = self.at(e, c);
        let n = self.normalized;
        match self.comp {
            ComponentType::F32 => f32::from_bits(self.word(at, 4)),
            ComponentType::U8 => {
                let v = self.word(at, 1) as f32;
                if n { v / 255.0 } else { v }
            }
            ComponentType::U16 => {
                let v = self.word(at, 2) as f32;
                if n { v / 65535.0 } else { v }
            }
            ComponentType::I8 => {
                let v = f32::from(self.word(at, 1) as u8 as i8);
                if n { (v / 127.0).max(-1.0) } else { v }
            }
            ComponentType::I16 => {
                let v = f32::from(self.word(at, 2) as u16 as i16);
                if n { (v / 32767.0).max(-1.0) } else { v }
            }
            ComponentType::U32 => self.word(at, 4) as f32,
        }
    }

    /// Component `c` of element `e` as an unsigned integer (None for a float accessor).
    pub fn u(&self, e: usize, c: usize) -> Option<u32> {
        if self.bytes.is_empty() {
            return Some(0);
        }
        let at = self.at(e, c);
        Some(match self.comp {
            ComponentType::U8 => self.word(at, 1),
            ComponentType::U16 => self.word(at, 2),
            ComponentType::U32 => self.word(at, 4),
            _ => return None,
        })
    }

    /// Every element as `N` numbers; the accessor must have `N` components.
    pub fn floats<const N: usize>(&self, what: &str) -> Result<Vec<[f32; N]>, ImportError> {
        if self.dims != N {
            return malformed(format!("{what} with {} components, not {N}", self.dims));
        }
        Ok((0..self.count)
            .map(|e| std::array::from_fn(|c| self.f(e, c)))
            .collect())
    }

    /// Every element as one unsigned integer (indices).
    pub fn indices(&self) -> Result<Vec<u32>, ImportError> {
        if self.dims != 1
            || matches!(
                self.comp,
                ComponentType::F32 | ComponentType::I8 | ComponentType::I16
            )
        {
            return malformed("indices that are not unsigned integers".into());
        }
        Ok((0..self.count).map(|e| self.u(e, 0).unwrap_or(0)).collect())
    }

    /// Every element as four unsigned integers (joint slots).
    pub fn joints(&self) -> Result<Vec<[u16; 4]>, ImportError> {
        if self.dims != 4 || !matches!(self.comp, ComponentType::U8 | ComponentType::U16) {
            return malformed("joints that are not four bytes or shorts".into());
        }
        Ok((0..self.count)
            .map(|e| std::array::from_fn(|c| self.u(e, c).unwrap_or(0) as u16))
            .collect())
    }

    /// Every element as a 4x4 matrix, turned from glTF's column order into rows.
    pub fn matrices(&self) -> Result<Vec<[f32; 16]>, ImportError> {
        if self.dims != 16 || self.comp != ComponentType::F32 {
            return malformed("matrices that are not 16 floats".into());
        }
        Ok((0..self.count)
            .map(|e| std::array::from_fn(|i| self.f(e, (i % 4) * 4 + i / 4)))
            .collect())
    }
}
