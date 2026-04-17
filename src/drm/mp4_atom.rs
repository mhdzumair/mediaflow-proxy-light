//! Minimal MP4 box (atom) parser.
//!
//! Ports `MP4Atom` and `MP4Parser` from `drm/decrypter.py`.

// ---------------------------------------------------------------------------
// Mp4Atom
// ---------------------------------------------------------------------------

/// A single MP4 box (atom): a 4-byte type tag plus its payload bytes.
#[derive(Debug, Clone)]
pub struct Mp4Atom {
    pub atom_type: [u8; 4],
    pub data: Vec<u8>,
}

impl Mp4Atom {
    pub fn new(atom_type: [u8; 4], data: Vec<u8>) -> Self {
        Self { atom_type, data }
    }

    /// Serialise the atom back to wire format: `size(4) || type(4) || data`.
    pub fn pack(&self) -> Vec<u8> {
        let size = (self.data.len() + 8) as u32;
        let mut out = Vec::with_capacity(8 + self.data.len());
        out.extend_from_slice(&size.to_be_bytes());
        out.extend_from_slice(&self.atom_type);
        out.extend_from_slice(&self.data);
        out
    }

    pub fn atom_type_str(&self) -> &str {
        std::str::from_utf8(&self.atom_type).unwrap_or("????")
    }
}

// ---------------------------------------------------------------------------
// Mp4Parser
// ---------------------------------------------------------------------------

/// Forward-only parser that reads [`Mp4Atom`]s from a byte buffer.
pub struct Mp4Parser {
    data: Vec<u8>,
    pub position: usize,
}

impl Mp4Parser {
    pub fn new(data: Vec<u8>) -> Self {
        Self { data, position: 0 }
    }

    /// Read the next atom from the current position.
    /// Returns `None` when no more data is available.
    pub fn read_atom(&mut self) -> Option<Mp4Atom> {
        self.read_atom_at(self.position, self.data.len())
            .map(|(atom, new_pos)| {
                self.position = new_pos;
                atom
            })
    }

    /// Collect all atoms at the top level (resets `position` to 0 first).
    pub fn list_atoms(&mut self) -> Vec<Mp4Atom> {
        let saved = self.position;
        self.position = 0;
        let mut atoms = Vec::new();
        while self.position + 8 <= self.data.len() {
            match self.read_atom() {
                Some(a) => atoms.push(a),
                None => break,
            }
        }
        self.position = saved;
        atoms
    }

    /// Parse a single atom starting at `pos`, bounded by `end`.
    /// Returns `(atom, next_pos)` or `None` on failure.
    fn read_atom_at(&self, pos: usize, end: usize) -> Option<(Mp4Atom, usize)> {
        if pos + 8 > end {
            return None;
        }
        let raw_size = u32::from_be_bytes(self.data[pos..pos + 4].try_into().ok()?) as usize;
        let mut atom_type = [0u8; 4];
        atom_type.copy_from_slice(&self.data[pos + 4..pos + 8]);
        let mut header_len = 8;

        let size = if raw_size == 1 {
            // 64-bit extended size
            if pos + 16 > end {
                return None;
            }
            header_len = 16;
            u64::from_be_bytes(self.data[pos + 8..pos + 16].try_into().ok()?) as usize
        } else if raw_size == 0 {
            // Atom extends to end of file
            end - pos
        } else {
            raw_size
        };

        if size < header_len || pos + size > end {
            return None;
        }

        let data_start = pos + header_len;
        let data_end = pos + size;
        let data = self.data[data_start..data_end].to_vec();
        let next_pos = pos + size;

        Some((Mp4Atom::new(atom_type, data), next_pos))
    }

    /// Collect all child atoms from a slice of `parent_data`.
    pub fn children_of(parent_data: &[u8]) -> Vec<Mp4Atom> {
        let mut parser = Mp4Parser::new(parent_data.to_vec());
        parser.list_atoms()
    }
}
