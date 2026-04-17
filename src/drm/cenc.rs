//! CENC / CBCS / CBC1 / CENS MP4 segment decryptor.
//!
//! Ports `MP4Decrypter` from `drm/decrypter.py`.

use std::collections::HashMap;

use tracing::warn;

use super::mp4_atom::{Mp4Atom, Mp4Parser};

// ---------------------------------------------------------------------------
// Sample auxiliary data
// ---------------------------------------------------------------------------

/// Per-sample encryption info parsed from a `senc` box.
#[derive(Debug, Clone)]
pub struct SampleInfo {
    pub is_encrypted: bool,
    /// Initialization vector (8 or 16 bytes).
    pub iv: Vec<u8>,
    /// Sub-sample list: (clear_bytes, encrypted_bytes).
    pub sub_samples: Vec<(u16, u32)>,
}

// ---------------------------------------------------------------------------
// Per-track info (collected during moof processing)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct TrackInfo {
    data_offset: i32,
    sample_sizes: Vec<u32>,
    sample_info: Vec<SampleInfo>,
    key: Option<Vec<u8>>,
    default_sample_size: u32,
    crypt_byte_block: u8,
    skip_byte_block: u8,
    constant_iv: Option<Vec<u8>>,
}

// ---------------------------------------------------------------------------
// Encryption scheme
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EncScheme {
    Cenc, // AES-CTR, full sample
    Cens, // AES-CTR, pattern
    Cbc1, // AES-CBC, full sample
    Cbcs, // AES-CBC, pattern
}

impl EncScheme {
    fn from_bytes(b: &[u8]) -> Self {
        match b {
            b"cbcs" => Self::Cbcs,
            b"cbc1" => Self::Cbc1,
            b"cens" => Self::Cens,
            _ => Self::Cenc,
        }
    }
}

// ---------------------------------------------------------------------------
// MP4Decrypter
// ---------------------------------------------------------------------------

pub struct Mp4Decrypter {
    key_map: HashMap<Vec<u8>, Vec<u8>>,
    // State accumulated during moof / stsd processing
    encryption_scheme: EncScheme,
    default_iv_size: usize,
    crypt_byte_block: u8,
    skip_byte_block: u8,
    constant_iv: Option<Vec<u8>>,
    default_sample_size: u32,
    trun_sample_sizes: Vec<u32>,
    current_sample_info: Vec<SampleInfo>,
    total_encryption_overhead: usize,
    track_infos: Vec<TrackInfo>,
    track_encryption_settings: HashMap<u32, TrackSettings>,
    extracted_kids: HashMap<u32, Vec<u8>>,
    current_track_id: u32,
}

#[derive(Debug, Clone)]
struct TrackSettings {
    crypt_byte_block: u8,
    skip_byte_block: u8,
    constant_iv: Option<Vec<u8>>,
    iv_size: usize,
}

impl Mp4Decrypter {
    pub fn new(key_map: HashMap<Vec<u8>, Vec<u8>>) -> Self {
        Self {
            key_map,
            encryption_scheme: EncScheme::Cenc,
            default_iv_size: 8,
            crypt_byte_block: 1,
            skip_byte_block: 9,
            constant_iv: None,
            default_sample_size: 0,
            trun_sample_sizes: Vec::new(),
            current_sample_info: Vec::new(),
            total_encryption_overhead: 0,
            track_infos: Vec::new(),
            track_encryption_settings: HashMap::new(),
            extracted_kids: HashMap::new(),
            current_track_id: 0,
        }
    }

    // -----------------------------------------------------------------------
    // Public entry points
    // -----------------------------------------------------------------------

    /// Decrypt a combined init+segment buffer.
    pub fn decrypt_segment(&mut self, combined: &[u8], include_init: bool) -> Vec<u8> {
        let mut parser = Mp4Parser::new(combined.to_vec());
        let atoms = parser.list_atoms();

        // Process in canonical order
        let mut processed: HashMap<[u8; 4], Mp4Atom> = HashMap::new();
        for type_tag in [*b"moov", *b"moof", *b"sidx", *b"mdat"] {
            if let Some(atom) = atoms.iter().find(|a| a.atom_type == type_tag) {
                let result = self.process_atom(atom);
                processed.insert(type_tag, result);
            }
        }

        let init_types: [&[u8; 4]; 2] = [b"ftyp", b"moov"];
        let mut out = Vec::new();
        for atom in &atoms {
            if !include_init && init_types.contains(&&atom.atom_type) {
                continue;
            }
            if let Some(p) = processed.get(&atom.atom_type) {
                out.extend_from_slice(&p.pack());
            } else {
                out.extend_from_slice(&atom.pack());
            }
        }
        out
    }

    /// Process only the init segment (remove encryption boxes from moov).
    pub fn process_init_only(&mut self, init: &[u8]) -> Vec<u8> {
        let mut parser = Mp4Parser::new(init.to_vec());
        let atoms = parser.list_atoms();

        let moov_processed = atoms
            .iter()
            .find(|a| a.atom_type == *b"moov")
            .map(|a| self.process_moov(a));

        let mut out = Vec::new();
        for atom in &atoms {
            if atom.atom_type == *b"moov" {
                if let Some(ref p) = moov_processed {
                    out.extend_from_slice(&p.pack());
                } else {
                    out.extend_from_slice(&atom.pack());
                }
            } else {
                out.extend_from_slice(&atom.pack());
            }
        }
        out
    }

    // -----------------------------------------------------------------------
    // Atom dispatch
    // -----------------------------------------------------------------------

    fn process_atom(&mut self, atom: &Mp4Atom) -> Mp4Atom {
        match &atom.atom_type {
            b"moov" => self.process_moov(atom),
            b"moof" => self.process_moof(atom),
            b"sidx" => self.process_sidx(atom),
            b"mdat" => self.decrypt_mdat(atom),
            _ => atom.clone(),
        }
    }

    // -----------------------------------------------------------------------
    // moov processing
    // -----------------------------------------------------------------------

    fn process_moov(&mut self, moov: &Mp4Atom) -> Mp4Atom {
        let children = Mp4Parser::children_of(&moov.data);
        let mut new_data = Vec::new();
        for child in &children {
            if child.atom_type == *b"trak" {
                let new_trak = self.process_trak(child);
                new_data.extend_from_slice(&new_trak.pack());
            } else if child.atom_type != *b"pssh" {
                new_data.extend_from_slice(&child.pack());
            }
        }
        Mp4Atom::new(*b"moov", new_data)
    }

    fn process_trak(&mut self, trak: &Mp4Atom) -> Mp4Atom {
        let children = Mp4Parser::children_of(&trak.data);

        // First pass: find track ID from tkhd
        for child in &children {
            if child.atom_type == *b"tkhd" {
                let version = child.data[0];
                let id_offset = if version == 0 { 12 } else { 20 };
                if child.data.len() >= id_offset + 4 {
                    self.current_track_id = u32::from_be_bytes(
                        child.data[id_offset..id_offset + 4]
                            .try_into()
                            .unwrap_or([0; 4]),
                    );
                }
                break;
            }
        }

        // Second pass: process atoms
        let mut new_data = Vec::new();
        for child in &children {
            if child.atom_type == *b"mdia" {
                new_data.extend_from_slice(&self.process_mdia(child).pack());
            } else {
                new_data.extend_from_slice(&child.pack());
            }
        }
        Mp4Atom::new(*b"trak", new_data)
    }

    fn process_mdia(&mut self, mdia: &Mp4Atom) -> Mp4Atom {
        let mut new_data = Vec::new();
        for child in Mp4Parser::children_of(&mdia.data) {
            if child.atom_type == *b"minf" {
                new_data.extend_from_slice(&self.process_minf(&child).pack());
            } else {
                new_data.extend_from_slice(&child.pack());
            }
        }
        Mp4Atom::new(*b"mdia", new_data)
    }

    fn process_minf(&mut self, minf: &Mp4Atom) -> Mp4Atom {
        let mut new_data = Vec::new();
        for child in Mp4Parser::children_of(&minf.data) {
            if child.atom_type == *b"stbl" {
                new_data.extend_from_slice(&self.process_stbl(&child).pack());
            } else {
                new_data.extend_from_slice(&child.pack());
            }
        }
        Mp4Atom::new(*b"minf", new_data)
    }

    fn process_stbl(&mut self, stbl: &Mp4Atom) -> Mp4Atom {
        let mut new_data = Vec::new();
        for child in Mp4Parser::children_of(&stbl.data) {
            if child.atom_type == *b"stsd" {
                new_data.extend_from_slice(&self.process_stsd(&child).pack());
            } else {
                new_data.extend_from_slice(&child.pack());
            }
        }
        Mp4Atom::new(*b"stbl", new_data)
    }

    fn process_stsd(&mut self, stsd: &Mp4Atom) -> Mp4Atom {
        if stsd.data.len() < 8 {
            return stsd.clone();
        }
        let entry_count = u32::from_be_bytes(stsd.data[4..8].try_into().unwrap_or([0; 4]));
        let mut new_data = stsd.data[..8].to_vec();

        let mut parser = Mp4Parser::new(stsd.data[8..].to_vec());
        for _ in 0..entry_count {
            if let Some(entry) = parser.read_atom() {
                let processed = self.process_sample_entry(&entry);
                new_data.extend_from_slice(&processed.pack());
            }
        }
        Mp4Atom::new(*b"stsd", new_data)
    }

    fn process_sample_entry(&mut self, entry: &Mp4Atom) -> Mp4Atom {
        // Fixed-field size depends on sample entry type
        let fixed_size: usize = match &entry.atom_type {
            b"mp4a" | b"enca" => 28,
            b"mp4v" | b"encv" | b"avc1" | b"hev1" | b"hvc1" => 78,
            _ => 16,
        };

        if entry.data.len() < fixed_size {
            return entry.clone();
        }

        let mut new_data = entry.data[..fixed_size.min(entry.data.len())].to_vec();
        let mut codec_format: Option<[u8; 4]> = None;

        let mut parser = Mp4Parser::new(entry.data[fixed_size..].to_vec());
        for child in parser.list_atoms() {
            match &child.atom_type {
                b"sinf" => {
                    codec_format = self.extract_codec_format(&child);
                }
                b"schi" | b"tenc" | b"schm" => { /* skip */ }
                _ => new_data.extend_from_slice(&child.pack()),
            }
        }

        let new_type = codec_format.unwrap_or(entry.atom_type);
        Mp4Atom::new(new_type, new_data)
    }

    fn extract_codec_format(&mut self, sinf: &Mp4Atom) -> Option<[u8; 4]> {
        let mut codec_format: Option<[u8; 4]> = None;
        for child in Mp4Parser::children_of(&sinf.data) {
            match &child.atom_type {
                b"frma" => {
                    if child.data.len() >= 4 {
                        let mut t = [0u8; 4];
                        t.copy_from_slice(&child.data[..4]);
                        codec_format = Some(t);
                    }
                }
                b"schm" => self.parse_schm(&child),
                b"schi" => {
                    for schi_child in Mp4Parser::children_of(&child.data) {
                        if schi_child.atom_type == *b"tenc" {
                            self.parse_tenc(&schi_child);
                        }
                    }
                }
                _ => {}
            }
        }
        codec_format
    }

    fn parse_schm(&mut self, schm: &Mp4Atom) {
        if schm.data.len() >= 8 {
            let scheme_type = &schm.data[4..8];
            self.encryption_scheme = EncScheme::from_bytes(scheme_type);
        }
    }

    fn parse_tenc(&mut self, tenc: &Mp4Atom) {
        let data = &tenc.data;
        if data.len() < 8 {
            return;
        }
        let version = data[0];
        let mut settings = TrackSettings {
            crypt_byte_block: 1,
            skip_byte_block: 9,
            constant_iv: None,
            iv_size: 8,
        };

        if version > 0 && data.len() >= 6 {
            let pattern_byte = data[5];
            settings.crypt_byte_block = (pattern_byte >> 4) & 0x0F;
            settings.skip_byte_block = pattern_byte & 0x0F;
            self.crypt_byte_block = settings.crypt_byte_block;
            self.skip_byte_block = settings.skip_byte_block;
        }

        // Extract KID (16 bytes at offset 8)
        if data.len() >= 24 {
            let kid = data[8..24].to_vec();
            if self.current_track_id > 0 {
                self.extracted_kids
                    .insert(self.current_track_id, kid.clone());
            }
        }

        // IV size at offset 7
        if data.len() > 7 {
            let iv_size = data[7] as usize;
            if iv_size == 0 || iv_size == 8 || iv_size == 16 {
                settings.iv_size = if iv_size > 0 { iv_size } else { 16 };
                self.default_iv_size = settings.iv_size;

                if iv_size == 0 && data.len() > 24 {
                    let civ_size_offset = 24;
                    let civ_size = data[civ_size_offset] as usize;
                    if civ_size > 0 && data.len() >= civ_size_offset + 1 + civ_size {
                        let civ =
                            data[civ_size_offset + 1..civ_size_offset + 1 + civ_size].to_vec();
                        settings.constant_iv = Some(civ.clone());
                        self.constant_iv = Some(civ);
                    }
                }
            }
        }

        if self.current_track_id > 0 {
            self.track_encryption_settings
                .insert(self.current_track_id, settings);
        }
    }

    // -----------------------------------------------------------------------
    // moof processing
    // -----------------------------------------------------------------------

    fn process_moof(&mut self, moof: &Mp4Atom) -> Mp4Atom {
        let children = Mp4Parser::children_of(&moof.data);
        self.track_infos.clear();

        // First pass: calculate total encryption overhead
        self.total_encryption_overhead = 0;
        for child in &children {
            if child.atom_type == *b"traf" {
                let traf_children = Mp4Parser::children_of(&child.data);
                let overhead: usize = traf_children
                    .iter()
                    .filter(|a| matches!(&a.atom_type, b"senc" | b"saiz" | b"saio"))
                    .map(|a| a.data.len() + 8)
                    .sum();
                self.total_encryption_overhead += overhead;
            }
        }

        // Second pass: process atoms
        let mut new_data = Vec::new();
        for child in &children {
            if child.atom_type == *b"traf" {
                new_data.extend_from_slice(&self.process_traf(child).pack());
            } else {
                new_data.extend_from_slice(&child.pack());
            }
        }
        Mp4Atom::new(*b"moof", new_data)
    }

    fn process_traf(&mut self, traf: &Mp4Atom) -> Mp4Atom {
        let children = Mp4Parser::children_of(&traf.data);
        let mut new_data = Vec::new();
        let mut tfhd_track_id: u32 = 0;
        let mut has_tfhd = false;
        let mut sample_count = 0u32;
        let mut trun_data_offset = 0i32;
        let mut sample_info: Vec<SampleInfo> = Vec::new();
        let mut track_default_sample_size = 0u32;

        for child in &children {
            match &child.atom_type {
                b"tfhd" => {
                    has_tfhd = true;
                    self.parse_tfhd(child);
                    track_default_sample_size = self.default_sample_size;
                    if child.data.len() >= 8 {
                        tfhd_track_id =
                            u32::from_be_bytes(child.data[4..8].try_into().unwrap_or([0; 4]));
                    }
                    new_data.extend_from_slice(&child.pack());
                }
                b"trun" => {
                    let (sc, doff) = self.process_trun(child);
                    sample_count = sc;
                    trun_data_offset = doff;
                    let modified = self.modify_trun(child);
                    new_data.extend_from_slice(&modified.pack());
                }
                b"senc" => {
                    sample_info = self.parse_senc(child, sample_count);
                    // senc is NOT written to new_data (stripped)
                }
                b"saiz" | b"saio" => { /* stripped */ }
                _ => new_data.extend_from_slice(&child.pack()),
            }
        }

        let track_key = self.get_key_for_track(tfhd_track_id);
        let track_enc = self.track_encryption_settings.get(&tfhd_track_id).cloned();
        let (crypt, skip, civ) = track_enc
            .as_ref()
            .map(|s| (s.crypt_byte_block, s.skip_byte_block, s.constant_iv.clone()))
            .unwrap_or((
                self.crypt_byte_block,
                self.skip_byte_block,
                self.constant_iv.clone(),
            ));

        self.track_infos.push(TrackInfo {
            data_offset: trun_data_offset,
            sample_sizes: self.trun_sample_sizes.clone(),
            sample_info: sample_info.clone(),
            key: track_key.cloned(),
            default_sample_size: track_default_sample_size,
            crypt_byte_block: crypt,
            skip_byte_block: skip,
            constant_iv: civ,
        });

        // Keep single-track compatibility fields
        self.current_sample_info = sample_info;

        // Some CMAF audio files omit tfhd and rely on trex defaults.
        // ffmpeg's mp4 parser requires tfhd to identify the track, so synthesize
        // a minimal one when none was present in the original traf.
        if !has_tfhd && self.current_track_id > 0 {
            let mut tfhd_data = vec![0u8; 8]; // version=0, flags=0, track_id
            tfhd_data[4..8].copy_from_slice(&self.current_track_id.to_be_bytes());
            let tfhd_atom = Mp4Atom::new(*b"tfhd", tfhd_data);
            // tfhd must precede trun inside traf
            let mut with_tfhd = tfhd_atom.pack();
            with_tfhd.extend_from_slice(&new_data);
            new_data = with_tfhd;
        }

        Mp4Atom::new(*b"traf", new_data)
    }

    fn parse_tfhd(&mut self, tfhd: &Mp4Atom) {
        let data = &tfhd.data;
        if data.len() < 8 {
            return;
        }
        let flags = (u32::from_be_bytes(data[0..4].try_into().unwrap_or([0; 4]))) & 0xFFFFFF;
        let mut offset = 8; // skip version_flags(4) + track_id(4)

        if flags & 0x000001 != 0 {
            offset += 8;
        } // base-data-offset
        if flags & 0x000002 != 0 {
            offset += 4;
        } // sample-description-index
        if flags & 0x000008 != 0 {
            offset += 4;
        } // default-sample-duration
        if flags & 0x000010 != 0 && offset + 4 <= data.len() {
            self.default_sample_size =
                u32::from_be_bytes(data[offset..offset + 4].try_into().unwrap_or([0; 4]));
        }
    }

    fn parse_senc(&self, senc: &Mp4Atom, _sample_count: u32) -> Vec<SampleInfo> {
        let data = &senc.data;
        if data.len() < 8 {
            return Vec::new();
        }
        let version_flags = u32::from_be_bytes(data[0..4].try_into().unwrap_or([0; 4]));
        let flags = version_flags & 0xFFFFFF;
        let mut pos = 4;

        // sample count from senc itself
        if data.len() < pos + 4 {
            return Vec::new();
        }
        let sc = u32::from_be_bytes(data[pos..pos + 4].try_into().unwrap_or([0; 4]));
        pos += 4;

        let iv_size = self.default_iv_size;
        let use_constant_iv =
            self.encryption_scheme == EncScheme::Cbcs && self.constant_iv.is_some();

        let mut samples = Vec::new();
        for _ in 0..sc {
            let iv = if use_constant_iv {
                self.constant_iv.clone().unwrap_or_default()
            } else {
                if pos + iv_size > data.len() {
                    break;
                }
                let iv_bytes = data[pos..pos + iv_size].to_vec();
                pos += iv_size;
                iv_bytes
            };

            let mut sub_samples = Vec::new();
            if flags & 0x000002 != 0 && pos + 2 <= data.len() {
                let ss_count = u16::from_be_bytes(data[pos..pos + 2].try_into().unwrap_or([0; 2]));
                pos += 2;
                for _ in 0..ss_count {
                    if pos + 6 <= data.len() {
                        let clear =
                            u16::from_be_bytes(data[pos..pos + 2].try_into().unwrap_or([0; 2]));
                        let enc =
                            u32::from_be_bytes(data[pos + 2..pos + 6].try_into().unwrap_or([0; 4]));
                        sub_samples.push((clear, enc));
                        pos += 6;
                    }
                }
            }

            samples.push(SampleInfo {
                is_encrypted: true,
                iv,
                sub_samples,
            });
        }
        samples
    }

    fn process_trun(&mut self, trun: &Mp4Atom) -> (u32, i32) {
        let data = &trun.data;
        if data.len() < 8 {
            return (0, 0);
        }
        let flags = u32::from_be_bytes(data[0..4].try_into().unwrap_or([0; 4]));
        let sample_count = u32::from_be_bytes(data[4..8].try_into().unwrap_or([0; 4]));
        let mut parse_offset = 8usize;

        let mut data_offset = 0i32;
        if flags & 0x000001 != 0 {
            if parse_offset + 4 <= data.len() {
                data_offset = i32::from_be_bytes(
                    data[parse_offset..parse_offset + 4]
                        .try_into()
                        .unwrap_or([0; 4]),
                );
            }
            parse_offset += 4;
        }
        if flags & 0x000004 != 0 {
            parse_offset += 4; // first-sample-flags
        }

        self.trun_sample_sizes.clear();
        for _ in 0..sample_count {
            if flags & 0x000100 != 0 {
                parse_offset += 4;
            } // sample-duration
            let size = if flags & 0x000200 != 0 {
                if parse_offset + 4 <= data.len() {
                    let s = u32::from_be_bytes(
                        data[parse_offset..parse_offset + 4]
                            .try_into()
                            .unwrap_or([0; 4]),
                    );
                    parse_offset += 4;
                    s
                } else {
                    parse_offset += 4;
                    0
                }
            } else {
                0
            };
            self.trun_sample_sizes.push(size);
            if flags & 0x000400 != 0 {
                parse_offset += 4;
            } // sample-flags
            if flags & 0x000800 != 0 {
                parse_offset += 4;
            } // composition-time-offset
        }

        (sample_count, data_offset)
    }

    fn modify_trun(&self, trun: &Mp4Atom) -> Mp4Atom {
        let mut data = trun.data.clone();
        if data.len() < 12 {
            return trun.clone();
        }
        let flags = (u32::from_be_bytes(data[0..4].try_into().unwrap_or([0; 4]))) & 0xFFFFFF;
        if flags & 0x000001 != 0 {
            let current = i32::from_be_bytes(data[8..12].try_into().unwrap_or([0; 4]));
            let new_offset = current - self.total_encryption_overhead as i32;
            data[8..12].copy_from_slice(&new_offset.to_be_bytes());
        }
        Mp4Atom::new(*b"trun", data)
    }

    fn process_sidx(&self, sidx: &Mp4Atom) -> Mp4Atom {
        let mut data = sidx.data.clone();
        if data.len() >= 36 {
            let current = u32::from_be_bytes(data[32..36].try_into().unwrap_or([0; 4]));
            let ref_type = current >> 31;
            let referenced_size =
                (current & 0x7FFFFFFF).saturating_sub(self.total_encryption_overhead as u32);
            let new_val = (ref_type << 31) | referenced_size;
            data[32..36].copy_from_slice(&new_val.to_be_bytes());
        }
        Mp4Atom::new(*b"sidx", data)
    }

    // -----------------------------------------------------------------------
    // mdat decryption
    // -----------------------------------------------------------------------

    fn decrypt_mdat(&mut self, mdat: &Mp4Atom) -> Mp4Atom {
        if !self.track_infos.is_empty() {
            return self.decrypt_mdat_multi_track(mdat);
        }

        // Single-track fallback
        let current_key = match self.get_current_key() {
            Some(k) => k.to_vec(),
            None => return mdat.clone(),
        };
        if self.current_sample_info.is_empty() {
            return mdat.clone();
        }

        let mdat_data = &mdat.data;
        let sample_info = self.current_sample_info.clone();
        let sample_sizes = self.trun_sample_sizes.clone();
        let default_size = self.default_sample_size;
        let scheme = self.encryption_scheme;
        let crypt = self.crypt_byte_block;
        let skip = self.skip_byte_block;
        let civ = self.constant_iv.clone();

        let mut decrypted = Vec::new();
        let mut pos = 0usize;

        for (i, info) in sample_info.iter().enumerate() {
            if pos >= mdat_data.len() {
                break;
            }
            let mut size = if i < sample_sizes.len() {
                sample_sizes[i] as usize
            } else {
                0
            };
            if size == 0 {
                size = if default_size > 0 {
                    default_size as usize
                } else {
                    mdat_data.len() - pos
                };
            }
            let sample = &mdat_data[pos..pos + size.min(mdat_data.len() - pos)];
            let dec = decrypt_sample(
                sample,
                info,
                &current_key,
                scheme,
                crypt,
                skip,
                civ.as_deref(),
            );
            decrypted.extend_from_slice(&dec);
            pos += size;
        }

        Mp4Atom::new(*b"mdat", decrypted)
    }

    fn decrypt_mdat_multi_track(&self, mdat: &Mp4Atom) -> Mp4Atom {
        if self.track_infos.is_empty() {
            return mdat.clone();
        }
        let mdat_data = &mdat.data;
        let mut decrypted = mdat_data.to_vec();

        // Sort tracks by data_offset
        let mut sorted_tracks = self.track_infos.clone();
        sorted_tracks.sort_by_key(|t| t.data_offset);

        let first_data_offset = sorted_tracks[0].data_offset;
        let scheme = self.encryption_scheme;

        for track in &sorted_tracks {
            let Some(ref key) = track.key else { continue };
            if track.sample_info.is_empty() {
                continue;
            }

            let mdat_pos_start = (track.data_offset - first_data_offset).max(0) as usize;
            let mut pos = mdat_pos_start;

            for (i, info) in track.sample_info.iter().enumerate() {
                let mut size = if i < track.sample_sizes.len() {
                    track.sample_sizes[i] as usize
                } else {
                    0
                };
                if size == 0 {
                    size = if track.default_sample_size > 0 {
                        track.default_sample_size as usize
                    } else {
                        continue;
                    };
                }
                if pos + size > mdat_data.len() {
                    break;
                }

                let sample = &mdat_data[pos..pos + size];
                let dec = decrypt_sample(
                    sample,
                    info,
                    key,
                    scheme,
                    track.crypt_byte_block,
                    track.skip_byte_block,
                    track.constant_iv.as_deref(),
                );
                decrypted[pos..pos + dec.len().min(size)]
                    .copy_from_slice(&dec[..dec.len().min(size)]);
                pos += size;
            }
        }

        Mp4Atom::new(*b"mdat", decrypted)
    }

    // -----------------------------------------------------------------------
    // Streaming helpers (public)
    // -----------------------------------------------------------------------

    /// Prime the decrypter from an init segment (moov box) without emitting bytes.
    /// Call this once before streaming moof/mdat pairs through `process_moof_atom`
    /// and `decrypt_mdat_atom`.
    pub fn init_from_segment(&mut self, init_bytes: &[u8]) {
        let mut parser = Mp4Parser::new(init_bytes.to_vec());
        for atom in parser.list_atoms() {
            if atom.atom_type == *b"moov" {
                self.process_moov(&atom); // side-effects only; return value discarded
            }
        }
    }

    /// Process a moof atom: strip senc/saiz/saio, populate per-sample info for
    /// the next `decrypt_mdat_atom` call.  Returns the cleaned moof atom.
    pub fn process_moof_atom(&mut self, atom: &Mp4Atom) -> Mp4Atom {
        self.process_moof(atom)
    }

    /// Decrypt a mdat atom using per-sample info gathered from the last
    /// `process_moof_atom` call.
    pub fn decrypt_mdat_atom(&mut self, atom: &Mp4Atom) -> Mp4Atom {
        self.decrypt_mdat(atom)
    }

    /// Adjust byte offsets in a sidx atom.
    pub fn process_sidx_atom(&self, atom: &Mp4Atom) -> Mp4Atom {
        self.process_sidx(atom)
    }

    // -----------------------------------------------------------------------
    // Key lookup
    // -----------------------------------------------------------------------

    fn get_key_for_track(&self, track_id: u32) -> Option<&Vec<u8>> {
        if let Some(kid) = self.extracted_kids.get(&track_id) {
            let all_zero = kid.iter().all(|&b| b == 0) && kid.len() == 16;
            if !all_zero {
                // Direct lookup: tenc KID matches a provided key_id byte-for-byte
                if let Some(k) = self.key_map.get(kid.as_slice()) {
                    return Some(k);
                }

                // PlayReady GUID fallback: some content packagers store the KID in
                // the tenc box using little-endian byte order for the first three UUID
                // components (PlayReady GUID format), while the MPD advertises
                // @cenc:default_KID in standard big-endian UUID order.
                //
                // UUID:    AABBCCDD-EEFF-GGHH-II...
                // LE GUID: DDCCBBAA-FFEE-HHGG-II...
                if kid.len() == 16 {
                    let mut swapped = kid.clone();
                    swapped[0..4].reverse(); // bytes 0-3
                    swapped[4..6].reverse(); // bytes 4-5
                    swapped[6..8].reverse(); // bytes 6-7
                                             // bytes 8-15 unchanged
                    if let Some(k) = self.key_map.get(swapped.as_slice()) {
                        return Some(k);
                    }
                }
            }
        }
        // Fallback: single key
        if self.key_map.len() == 1 {
            return self.key_map.values().next();
        }
        None
    }

    fn get_current_key(&self) -> Option<&Vec<u8>> {
        if self.key_map.len() == 1 {
            return self.key_map.values().next();
        }
        None
    }
}

// ---------------------------------------------------------------------------
// Sample decryption (free functions)
// ---------------------------------------------------------------------------

fn decrypt_sample(
    sample: &[u8],
    info: &SampleInfo,
    key: &[u8],
    scheme: EncScheme,
    crypt_byte_block: u8,
    skip_byte_block: u8,
    constant_iv: Option<&[u8]>,
) -> Vec<u8> {
    if !info.is_encrypted {
        return sample.to_vec();
    }
    match scheme {
        EncScheme::Cbcs => decrypt_sample_cbcs(
            sample,
            info,
            key,
            crypt_byte_block,
            skip_byte_block,
            constant_iv,
        ),
        EncScheme::Cbc1 => decrypt_sample_cbc1(sample, info, key),
        _ => decrypt_sample_cenc(sample, info, key),
    }
}

/// AES-CTR (cenc / cens).
fn decrypt_sample_cenc(sample: &[u8], info: &SampleInfo, key: &[u8]) -> Vec<u8> {
    type Aes128Ctr = ctr::Ctr128BE<aes::Aes128>;
    type Aes256Ctr = ctr::Ctr128BE<aes::Aes256>;

    let mut iv = [0u8; 16];
    let copy_len = info.iv.len().min(16);
    iv[..copy_len].copy_from_slice(&info.iv[..copy_len]);

    if info.sub_samples.is_empty() {
        return aes_ctr_decrypt(key, &iv, sample);
    }

    let mut result = Vec::with_capacity(sample.len());
    let mut pos = 0usize;
    // Create a fresh cipher for each sub-sample group (IV does not reset between subsamples)
    let mut offset_blocks = 0u128;

    for &(clear, enc) in &info.sub_samples {
        // Copy clear bytes
        let end = (pos + clear as usize).min(sample.len());
        result.extend_from_slice(&sample[pos..end]);
        pos += clear as usize;

        // Decrypt encrypted bytes
        let enc_end = (pos + enc as usize).min(sample.len());
        let enc_data = &sample[pos..enc_end];

        // AES-CTR: the block counter increments by 1 per 16-byte block
        // We need to continue from where we left off
        let decrypted = aes_ctr_decrypt_with_offset(key, &iv, enc_data, offset_blocks);
        offset_blocks += (enc as u128).div_ceil(16);
        result.extend_from_slice(&decrypted);
        pos += enc as usize;
    }

    // Remaining data
    if pos < sample.len() {
        let enc_data = &sample[pos..];
        let decrypted = aes_ctr_decrypt_with_offset(key, &iv, enc_data, offset_blocks);
        result.extend_from_slice(&decrypted);
    }

    result
}

fn aes_ctr_decrypt(key: &[u8], iv: &[u8; 16], data: &[u8]) -> Vec<u8> {
    aes_ctr_decrypt_with_offset(key, iv, data, 0)
}

fn aes_ctr_decrypt_with_offset(
    key: &[u8],
    iv: &[u8; 16],
    data: &[u8],
    block_offset: u128,
) -> Vec<u8> {
    use aes::cipher::{KeyIvInit, StreamCipher};

    // Adjust IV by adding block_offset to the counter portion (last 8 bytes in big-endian)
    let mut iv_with_offset = *iv;
    if block_offset > 0 {
        let counter = u64::from_be_bytes(iv_with_offset[8..].try_into().unwrap_or([0; 8]));
        let new_counter = counter.wrapping_add(block_offset as u64);
        iv_with_offset[8..].copy_from_slice(&new_counter.to_be_bytes());
    }

    let mut buf = data.to_vec();
    match key.len() {
        16 => {
            type Aes128Ctr = ctr::Ctr128BE<aes::Aes128>;
            if let Ok(mut cipher) = Aes128Ctr::new_from_slices(key, &iv_with_offset) {
                cipher.apply_keystream(&mut buf);
            }
        }
        32 => {
            type Aes256Ctr = ctr::Ctr128BE<aes::Aes256>;
            if let Ok(mut cipher) = Aes256Ctr::new_from_slices(key, &iv_with_offset) {
                cipher.apply_keystream(&mut buf);
            }
        }
        _ => {
            warn!("Unsupported AES key length: {}", key.len());
        }
    }
    buf
}

/// AES-CBC (cbc1): per-sample, full blocks only.
fn decrypt_sample_cbc1(sample: &[u8], info: &SampleInfo, key: &[u8]) -> Vec<u8> {
    let mut iv = [0u8; 16];
    let copy_len = info.iv.len().min(16);
    iv[..copy_len].copy_from_slice(&info.iv[..copy_len]);

    if info.sub_samples.is_empty() {
        return aes_cbc_decrypt_full_blocks(key, &iv, sample);
    }

    let mut result = Vec::with_capacity(sample.len());
    let mut pos = 0usize;

    for &(clear, enc) in &info.sub_samples {
        let end = (pos + clear as usize).min(sample.len());
        result.extend_from_slice(&sample[pos..end]);
        pos += clear as usize;

        let enc_end = (pos + enc as usize).min(sample.len());
        let enc_data = &sample[pos..enc_end];
        result.extend_from_slice(&aes_cbc_decrypt_full_blocks(key, &iv, enc_data));
        pos += enc as usize;
    }
    if pos < sample.len() {
        result.extend_from_slice(&sample[pos..]);
    }
    result
}

/// AES-CBC with pattern encryption (cbcs).
fn decrypt_sample_cbcs(
    sample: &[u8],
    info: &SampleInfo,
    key: &[u8],
    crypt_byte_block: u8,
    skip_byte_block: u8,
    constant_iv: Option<&[u8]>,
) -> Vec<u8> {
    let mut iv = [0u8; 16];
    if let Some(civ) = constant_iv {
        let copy_len = civ.len().min(16);
        iv[..copy_len].copy_from_slice(&civ[..copy_len]);
    } else {
        let copy_len = info.iv.len().min(16);
        iv[..copy_len].copy_from_slice(&info.iv[..copy_len]);
    }

    if info.sub_samples.is_empty() {
        return decrypt_cbcs_pattern(sample, key, &iv, crypt_byte_block, skip_byte_block);
    }

    let mut result = Vec::with_capacity(sample.len());
    let mut pos = 0usize;

    for &(clear, enc) in &info.sub_samples {
        let end = (pos + clear as usize).min(sample.len());
        result.extend_from_slice(&sample[pos..end]);
        pos += clear as usize;

        let enc_end = (pos + enc as usize).min(sample.len());
        let enc_data = &sample[pos..enc_end];
        result.extend_from_slice(&decrypt_cbcs_pattern(
            enc_data,
            key,
            &iv,
            crypt_byte_block,
            skip_byte_block,
        ));
        pos += enc as usize;
    }
    if pos < sample.len() {
        result.extend_from_slice(&sample[pos..]);
    }
    result
}

/// CBCS pattern decryption: crypt N blocks, skip M blocks, repeat.
fn decrypt_cbcs_pattern(
    data: &[u8],
    key: &[u8],
    iv: &[u8; 16],
    crypt_blocks: u8,
    skip_blocks: u8,
) -> Vec<u8> {
    if data.is_empty() || crypt_blocks == 0 {
        return data.to_vec();
    }

    let block_size = 16usize;
    let crypt_bytes = crypt_blocks as usize * block_size;
    let skip_bytes = skip_blocks as usize * block_size;

    // If skip=0, full block encryption
    if skip_bytes == 0 {
        return aes_cbc_decrypt_full_blocks(key, iv, data);
    }

    // Collect all encrypted blocks in order, tracking their positions
    let mut encrypted_blocks = Vec::new();
    let mut block_positions: Vec<(usize, usize)> = Vec::new(); // (orig_offset, length)
    let mut pos = 0usize;

    while pos < data.len() {
        // Encrypted portion
        if pos + crypt_bytes <= data.len() {
            encrypted_blocks.extend_from_slice(&data[pos..pos + crypt_bytes]);
            block_positions.push((pos, crypt_bytes));
            pos += crypt_bytes;
        } else {
            let remaining = data.len() - pos;
            let complete = (remaining / block_size) * block_size;
            if complete > 0 {
                encrypted_blocks.extend_from_slice(&data[pos..pos + complete]);
                block_positions.push((pos, complete));
            }
            break;
        }

        // Skip portion
        if pos + skip_bytes <= data.len() {
            pos += skip_bytes;
        } else {
            break;
        }
    }

    if encrypted_blocks.is_empty() {
        return data.to_vec();
    }

    // Decrypt all encrypted blocks as a continuous CBC stream
    let decrypted_blocks = aes_cbc_decrypt_full_blocks(key, iv, &encrypted_blocks);

    // Reconstruct output
    let mut result = data.to_vec();
    let mut dec_pos = 0usize;
    for (orig_pos, length) in &block_positions {
        result[*orig_pos..*orig_pos + length]
            .copy_from_slice(&decrypted_blocks[dec_pos..dec_pos + length]);
        dec_pos += length;
    }
    result
}

/// Decrypt complete 16-byte blocks with AES-CBC (leave trailing partial block unchanged).
fn aes_cbc_decrypt_full_blocks(key: &[u8], iv: &[u8; 16], data: &[u8]) -> Vec<u8> {
    use aes::cipher::{block_padding::NoPadding, BlockDecryptMut, KeyIvInit};

    let complete = (data.len() / 16) * 16;
    if complete == 0 {
        return data.to_vec();
    }

    let mut result = data.to_vec();
    match key.len() {
        16 => {
            type Aes128CbcDec = cbc::Decryptor<aes::Aes128>;
            if let Ok(cipher) = Aes128CbcDec::new_from_slices(key, iv) {
                if let Ok(out) = cipher.decrypt_padded_mut::<NoPadding>(&mut result[..complete]) {
                    // out is a slice into result, already decrypted in-place
                    let _ = out;
                }
            }
        }
        32 => {
            type Aes256CbcDec = cbc::Decryptor<aes::Aes256>;
            if let Ok(cipher) = Aes256CbcDec::new_from_slices(key, iv) {
                if let Ok(out) = cipher.decrypt_padded_mut::<NoPadding>(&mut result[..complete]) {
                    let _ = out;
                }
            }
        }
        _ => {
            warn!("Unsupported AES key length: {}", key.len());
        }
    }
    result
}

// ---------------------------------------------------------------------------
// Streaming CENC decryption
// ---------------------------------------------------------------------------

/// Stream-decrypt a CENC-protected SegmentBase media file.
///
/// `init_bytes`     – The init segment (moov box, bytes 0..init_range_end).
/// `segment_stream` – Streaming bytes from the CDN (bytes init_range_end+1 to end).
/// `key_id` / `key` – Possibly comma-separated hex strings for multi-key DRM.
///
/// Returns a `Stream` that yields decrypted fMP4 chunks as each moof+mdat pair
/// is processed — the client (mpv/ffmpeg) can start decoding before the full
/// file is downloaded.
pub fn decrypt_segment_streaming<S>(
    init_bytes: bytes::Bytes,
    segment_stream: S,
    key_id: String,
    key: String,
    // When true (no separate EXT-X-MAP), emit the processed moov/ftyp as the
    // first chunk so the player receives a self-contained fMP4 stream.
    include_init: bool,
) -> impl futures::Stream<Item = Result<bytes::Bytes, crate::error::AppError>>
where
    S: futures::Stream<Item = Result<bytes::Bytes, crate::error::AppError>>,
{
    use crate::error::AppError;
    use futures::StreamExt;

    let key_map = crate::drm::clearkey::build_key_map_from_hex(&key_id, &key);

    async_stream::stream! {
        if key_map.is_empty() {
            yield Err(AppError::Drm("No valid key/key_id pairs provided for streaming".to_string()));
            return;
        }

        let mut decrypter = Mp4Decrypter::new(key_map);

        if include_init {
            // process_init_only both primes the decrypter state AND strips the
            // encryption boxes from moov.  Emit the cleaned init bytes first so
            // the player has context before any moof/mdat arrives.
            let cleaned_init = decrypter.process_init_only(&init_bytes);
            yield Ok(bytes::Bytes::from(cleaned_init));
        } else {
            // Init sent separately (EXT-X-MAP) — only prime the decrypter state.
            decrypter.init_from_segment(&init_bytes);
        }

        // Rolling byte buffer for partial box data.
        let mut buf: Vec<u8> = Vec::new();

        // Current box we are accumulating.
        let mut cur_type: Option<[u8; 4]> = None;
        let mut cur_body: Vec<u8> = Vec::new();
        let mut cur_body_remaining: usize = 0;

        let mut stream = std::pin::pin!(segment_stream);

        'outer: loop {
            // ----------------------------------------------------------------
            // Process as many complete boxes as possible from `buf`.
            // ----------------------------------------------------------------
            loop {
                if cur_type.is_none() {
                    // Need at least 8 bytes to read the standard box header.
                    if buf.len() < 8 {
                        break;
                    }
                    let size_u32 = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]);
                    let btype: [u8; 4] = [buf[4], buf[5], buf[6], buf[7]];

                    let (header_len, body_len) = if size_u32 == 1 {
                        // 64-bit extended size in the next 8 bytes.
                        if buf.len() < 16 {
                            break; // wait for more data
                        }
                        let ext = u64::from_be_bytes(
                            buf[8..16].try_into().unwrap_or([0; 8]),
                        ) as usize;
                        (16usize, ext.saturating_sub(16))
                    } else if size_u32 == 0 {
                        // Box extends to the end of the stream.
                        // Pass through the header and let the rest fall
                        // through unchanged at the end of the outer loop.
                        let hdr = buf[..8].to_vec();
                        buf.drain(..8);
                        yield Ok(bytes::Bytes::from(hdr));
                        // Drain any already-buffered bytes.
                        if !buf.is_empty() {
                            yield Ok(bytes::Bytes::from(std::mem::take(&mut buf)));
                        }
                        // Forward the rest of the stream unmodified.
                        while let Some(chunk) = stream.next().await {
                            match chunk {
                                Ok(c) => yield Ok(c),
                                Err(e) => { yield Err(e); return; }
                            }
                        }
                        return;
                    } else {
                        (8usize, (size_u32 as usize).saturating_sub(8))
                    };

                    buf.drain(..header_len);
                    cur_type = Some(btype);
                    cur_body_remaining = body_len;
                    cur_body.clear();
                    // Pre-reserve so realloc pressure is low for small boxes.
                    cur_body.reserve(body_len.min(256 * 1024));
                }

                // Accumulate body bytes from the buffer.
                let take = buf.len().min(cur_body_remaining);
                if take > 0 {
                    cur_body.extend_from_slice(&buf[..take]);
                    buf.drain(..take);
                    cur_body_remaining -= take;
                }

                if cur_body_remaining > 0 {
                    break; // need more incoming data
                }

                // ----------------------------------------------------------------
                // Box is complete.  Process it.
                // ----------------------------------------------------------------
                let btype = cur_type.take().unwrap();
                let atom = Mp4Atom::new(btype, std::mem::take(&mut cur_body));

                // Drop sidx: its byte-offset references point to the original
                // encrypted stream and become incorrect after senc/saiz/saio
                // stripping. Omitting it lets the demuxer fall back to scanning
                // moof boxes sequentially, which is always correct.
                if &btype == b"sidx" {
                    continue;
                }

                let output = match &btype {
                    b"moof" => decrypter.process_moof_atom(&atom),
                    b"mdat" => decrypter.decrypt_mdat_atom(&atom),
                    _ => atom, // styp, free, etc. — pass through unchanged
                };

                yield Ok(bytes::Bytes::from(output.pack()));
            }

            // ----------------------------------------------------------------
            // Read the next chunk from the upstream CDN.
            // ----------------------------------------------------------------
            match stream.next().await {
                Some(Ok(chunk)) => buf.extend_from_slice(&chunk),
                Some(Err(e)) => {
                    yield Err(e);
                    return;
                }
                None => {
                    // Stream finished.  If a box was still partially buffered,
                    // log a warning (the content is probably corrupt/truncated).
                    if cur_type.is_some() || cur_body_remaining > 0 {
                        tracing::warn!(
                            "CENC stream ended with partial box: {} bytes buffered, {} body remaining",
                            buf.len(), cur_body_remaining,
                        );
                    }
                    break 'outer;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Public entry points (mirrors Python module-level functions)
// ---------------------------------------------------------------------------

/// Decrypt a CENC-protected segment.
///
/// `key_id` and `key` may be comma-separated hex strings for multi-key DRM.
pub fn decrypt_segment(
    init_segment: &[u8],
    segment_content: &[u8],
    key_id: &str,
    key: &str,
    include_init: bool,
) -> Result<Vec<u8>, String> {
    let key_map = crate::drm::clearkey::build_key_map_from_hex(key_id, key);
    if key_map.is_empty() {
        return Err("No valid key/key_id pairs provided".to_string());
    }
    let mut decrypter = Mp4Decrypter::new(key_map);
    let mut combined = Vec::with_capacity(init_segment.len() + segment_content.len());
    combined.extend_from_slice(init_segment);
    combined.extend_from_slice(segment_content);
    Ok(decrypter.decrypt_segment(&combined, include_init))
}

/// Strip encryption boxes from an init segment (for EXT-X-MAP use).
pub fn process_drm_init_segment(
    init_segment: &[u8],
    key_id: &str,
    key: &str,
) -> Result<Vec<u8>, String> {
    let key_map = crate::drm::clearkey::build_key_map_from_hex(key_id, key);
    if key_map.is_empty() {
        return Err("No valid key/key_id pairs provided".to_string());
    }
    let mut decrypter = Mp4Decrypter::new(key_map);
    Ok(decrypter.process_init_only(init_segment))
}
