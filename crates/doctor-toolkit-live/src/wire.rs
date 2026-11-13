//! Little-endian fields of a payload.

use crate::proto::DecodeError;

#[derive(Default)]
pub(crate) struct Writer {
    pub(crate) buf: Vec<u8>,
}

impl Writer {
    pub(crate) fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }
    pub(crate) fn u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    pub(crate) fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    pub(crate) fn u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    pub(crate) fn f32(&mut self, v: f32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    pub(crate) fn opt_u32(&mut self, v: Option<u32>) {
        match v {
            Some(x) => {
                self.u8(1);
                self.u32(x);
            }
            None => self.u8(0),
        }
    }
    pub(crate) fn bool(&mut self, v: bool) {
        self.u8(u8::from(v));
    }
    pub(crate) fn vec3(&mut self, v: &[f32; 3]) {
        for x in v {
            self.f32(*x);
        }
    }
    pub(crate) fn bytes(&mut self, v: &[u8]) {
        self.buf.extend_from_slice(v);
    }
    /// A string: u16 length and UTF-8 bytes, cut at 65 535 bytes on a
    /// character boundary.
    pub(crate) fn str(&mut self, v: &str) {
        let mut end = v.len().min(u16::MAX as usize);
        while !v.is_char_boundary(end) {
            end -= 1;
        }
        self.u16(end as u16);
        self.bytes(&v.as_bytes()[..end]);
    }
    pub(crate) fn opt_f32(&mut self, v: Option<f32>) {
        match v {
            Some(x) => {
                self.u8(1);
                self.f32(x);
            }
            None => self.u8(0),
        }
    }
}

pub(crate) struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Self { data, at: 0 }
    }
    pub(crate) fn take(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        let end = self.at.checked_add(n).ok_or(DecodeError::Truncated)?;
        let out = self.data.get(self.at..end).ok_or(DecodeError::Truncated)?;
        self.at = end;
        Ok(out)
    }
    pub(crate) fn u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.take(1)?[0])
    }
    pub(crate) fn u16(&mut self) -> Result<u16, DecodeError> {
        Ok(u16::from_le_bytes(
            self.take(2)?.try_into().unwrap_or_default(),
        ))
    }
    pub(crate) fn u32(&mut self) -> Result<u32, DecodeError> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().unwrap_or_default(),
        ))
    }
    pub(crate) fn u64(&mut self) -> Result<u64, DecodeError> {
        Ok(u64::from_le_bytes(
            self.take(8)?.try_into().unwrap_or_default(),
        ))
    }
    pub(crate) fn f32(&mut self) -> Result<f32, DecodeError> {
        Ok(f32::from_bits(self.u32()?))
    }
    pub(crate) fn opt_u32(&mut self) -> Result<Option<u32>, DecodeError> {
        Ok(if self.bool()? {
            Some(self.u32()?)
        } else {
            None
        })
    }
    pub(crate) fn bool(&mut self) -> Result<bool, DecodeError> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(DecodeError::BadValue("bool")),
        }
    }
    pub(crate) fn vec3(&mut self) -> Result<[f32; 3], DecodeError> {
        Ok([self.f32()?, self.f32()?, self.f32()?])
    }
    pub(crate) fn guid(&mut self) -> Result<[u8; 16], DecodeError> {
        let mut g = [0u8; 16];
        g.copy_from_slice(self.take(16)?);
        Ok(g)
    }
    pub(crate) fn str(&mut self) -> Result<String, DecodeError> {
        let n = self.u16()? as usize;
        let bytes = self.take(n)?;
        String::from_utf8(bytes.to_vec()).map_err(|_| DecodeError::BadUtf8)
    }
    pub(crate) fn opt_f32(&mut self) -> Result<Option<f32>, DecodeError> {
        Ok(if self.bool()? {
            Some(self.f32()?)
        } else {
            None
        })
    }
    /// A list count, refused when the rest of the payload cannot hold that
    /// many items of at least `min` bytes (a broken count must not make the
    /// reader allocate gigabytes).
    pub(crate) fn count(&mut self, min: usize) -> Result<usize, DecodeError> {
        let n = self.u32()? as usize;
        if n.saturating_mul(min.max(1)) > self.data.len() - self.at {
            return Err(DecodeError::Truncated);
        }
        Ok(n)
    }
    pub(crate) fn finish(self) -> Result<(), DecodeError> {
        match self.data.len() - self.at {
            0 => Ok(()),
            n => Err(DecodeError::Trailing(n)),
        }
    }
}
