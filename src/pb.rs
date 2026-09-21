//! A minimal protocol buffers reader, shared by the GTFS-Realtime and vector
//! tile decoders. Both formats are stable and use a handful of field types, so
//! this avoids a code generator and a `protoc` build dependency.

pub struct Buf<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Buf<'a> {
    pub fn new(b: &'a [u8]) -> Self {
        Buf { b, i: 0 }
    }

    pub fn done(&self) -> bool {
        self.i >= self.b.len()
    }

    pub fn varint(&mut self) -> Option<u64> {
        let mut v = 0u64;
        let mut shift = 0u32;
        loop {
            let byte = *self.b.get(self.i)?;
            self.i += 1;
            v |= ((byte & 0x7f) as u64) << shift;
            if byte & 0x80 == 0 {
                return Some(v);
            }
            shift += 7;
            if shift > 63 {
                return None;
            }
        }
    }

    /// Returns `(field_number, wire_type)`.
    pub fn tag(&mut self) -> Option<(u32, u8)> {
        let key = self.varint()?;
        Some(((key >> 3) as u32, (key & 7) as u8))
    }

    pub fn bytes(&mut self) -> Option<&'a [u8]> {
        let len = self.varint()? as usize;
        let end = self.i.checked_add(len)?;
        let out = self.b.get(self.i..end)?;
        self.i = end;
        Some(out)
    }

    pub fn fixed32(&mut self) -> Option<u32> {
        let end = self.i.checked_add(4)?;
        let s = self.b.get(self.i..end)?;
        self.i = end;
        Some(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }

    pub fn fixed64(&mut self) -> Option<u64> {
        let end = self.i.checked_add(8)?;
        let s = self.b.get(self.i..end)?;
        self.i = end;
        Some(u64::from_le_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]]))
    }

    pub fn skip(&mut self, wire: u8) -> Option<()> {
        match wire {
            0 => { self.varint()?; }
            1 => { self.fixed64()?; }
            2 => { self.bytes()?; }
            5 => { self.fixed32()?; }
            _ => return None,
        }
        Some(())
    }

    /// Every varint in a packed repeated field.
    pub fn packed(b: &'a [u8]) -> Vec<u32> {
        let mut p = Buf::new(b);
        let mut out = Vec::new();
        while !p.done() {
            match p.varint() {
                Some(v) => out.push(v as u32),
                None => break,
            }
        }
        out
    }
}
